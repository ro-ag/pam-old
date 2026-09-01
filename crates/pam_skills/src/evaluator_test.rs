#![cfg(unix)]

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use super::evaluator::{
    DetectedEvaluator, EvaluatorKind, EvaluatorRunConfig, EvaluatorRunError, detect_evaluator,
    retained_evaluator_auth_environment, run_evaluator,
};

static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(name: &str) -> Self {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "pam-skills-evaluator-test-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn directory(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

fn write_stub(directory: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn joined_path(paths: &[&Path]) -> OsString {
    env::join_paths(paths).unwrap()
}

fn detect_single(bin: &Path, project: &Path) -> DetectedEvaluator {
    detect_evaluator(
        &joined_path(&[bin, Path::new("/bin"), Path::new("/usr/bin")]),
        Some(project),
    )
    .unwrap()
    .unwrap()
}

/// How long a test waits for the shell stub standing in for the evaluator.
///
/// Client patience, not an assertion: these tests assert what the stub wrote
/// and what came back, never how long it took. The stub itself is `printf` and
/// finishes in milliseconds — but `run_evaluator` spawns a real process, pipes
/// a prompt into it and reaps it, and on an oversubscribed runner every one of
/// those steps queues behind everybody else's. Fifteen seconds looked
/// enormously generous and still lost: under 24 concurrent copies of this test
/// binary these runs returned `DeadlineExceeded`. Forty-five seconds is just as
/// free while the machine is healthy, because a healthy run never reaches it.
///
/// The one deadline in this file that is deliberately tiny lives in
/// `elapsed_deadline_kills_and_reaps_the_child`, which asserts the timeout
/// fires. That one is an assertion and must stay where it is.
const STUB_DEADLINE: Duration = Duration::from_secs(45);

fn test_config(
    deadline: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
) -> EvaluatorRunConfig {
    EvaluatorRunConfig::new(deadline, 1024, max_stdout_bytes, max_stderr_bytes).unwrap()
}

#[test]
fn detection_ignores_unsupported_headless_clients_and_selects_claude() {
    let test = TestDirectory::new("precedence");
    let project = test.directory("project");
    let first = test.directory("first-bin");
    let second = test.directory("second-bin");
    write_stub(&first, "cursor-agent", "printf cursor");
    write_stub(&first, "codex", "printf codex");
    let claude = write_stub(&second, "claude", "printf claude");

    let detected = detect_evaluator(&joined_path(&[&first, &second]), Some(&project))
        .unwrap()
        .unwrap();

    assert_eq!(detected.kind(), EvaluatorKind::Claude);
    assert_eq!(detected.executable(), fs::canonicalize(claude).unwrap());
    assert!(!format!("{detected:?}").contains(second.to_str().unwrap()));
}

#[test]
fn codex_and_cursor_are_not_detected_without_verified_tool_free_modes() {
    let test = TestDirectory::new("unsupported-clients");
    let project = test.directory("project");
    let bin = test.directory("bin");
    write_stub(&bin, "codex", "printf should-not-run");
    write_stub(&bin, "cursor-agent", "printf should-not-run");

    assert!(
        detect_evaluator(&joined_path(&[&bin]), Some(&project))
            .unwrap()
            .is_none()
    );
}

#[test]
fn relative_path_entries_and_non_executable_files_are_ignored() {
    let test = TestDirectory::new("relative-path");
    let project = test.directory("project");
    let relative_bin = test.directory("relative-bin");
    write_stub(&relative_bin, "claude", "printf should-not-run");
    let non_executable_bin = test.directory("non-executable-bin");
    fs::write(non_executable_bin.join("claude"), "not executable").unwrap();
    let relative_bin = relative_bin
        .strip_prefix(test.path.parent().unwrap())
        .unwrap();

    assert!(
        detect_evaluator(
            &joined_path(&[relative_bin, &non_executable_bin]),
            Some(&project)
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn executables_inside_the_audited_project_are_rejected() {
    let test = TestDirectory::new("project-contained");
    let project = test.directory("project");
    let bin = project.join("bin");
    fs::create_dir(&bin).unwrap();
    write_stub(&bin, "claude", "printf should-not-run");

    assert!(
        detect_evaluator(&joined_path(&[&bin]), Some(&project))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_global_audit_without_an_audited_project_keeps_every_path_entry() {
    let test = TestDirectory::new("no-audited-project");
    let home = test.directory("home");
    let claude_bin = home.join(".claude/local");
    let shim_bin = home.join(".local/bin");
    fs::create_dir_all(&claude_bin).unwrap();
    fs::create_dir_all(&shim_bin).unwrap();
    let shim_sentinel = test.file("shim-ran");
    write_stub(
        &shim_bin,
        "pam-shim-helper",
        &format!(": > {}", shell_path(&shim_sentinel)),
    );
    let claude = write_stub(
        &claude_bin,
        "claude",
        "pam-shim-helper\nprintf 'stub response'",
    );
    let path = joined_path(&[&claude_bin, &shim_bin]);

    // Audited as a project, this whole tree is distrusted and nothing is found.
    assert!(detect_evaluator(&path, Some(&home)).unwrap().is_none());

    let detected = detect_evaluator(&path, None).unwrap().unwrap();
    let response = run_evaluator(
        &detected,
        "private prompt over stdin",
        test_config(STUB_DEADLINE, 1024, 1024),
    )
    .unwrap();

    assert_eq!(detected.executable(), fs::canonicalize(claude).unwrap());
    assert_eq!(response, "stub response");
    // The sanitized PATH handed to the evaluator still reaches every entry.
    assert!(shim_sentinel.exists());
}

#[test]
fn claude_runs_once_with_no_tools_sanitized_path_stdin_and_an_empty_workspace() {
    let test = TestDirectory::new("invocation");
    let project = test.directory("project");
    let evaluator_bin = test.directory("evaluator-bin");
    let project_bin = project.join("bin");
    fs::create_dir(&project_bin).unwrap();
    let helper_sentinel = test.file("project-helper-ran");
    write_stub(
        &project_bin,
        "pam-project-helper",
        &format!(": > {}", shell_path(&helper_sentinel)),
    );
    let invocation_log = test.file("invocations");
    let arguments_log = test.file("arguments");
    let stdin_log = test.file("stdin");
    let cwd_log = test.file("cwd");
    let entries_log = test.file("entries");
    let environment_leak_log = test.file("environment-leak");
    let body = format!(
        "if command -v pam-project-helper >/dev/null 2>&1; then pam-project-helper; exit 90; fi\n\
         if [ \"${{NODE_OPTIONS+x}}\" = x ] || [ \"${{PYTHONPATH+x}}\" = x ] || \
            [ \"${{PYTHONHOME+x}}\" = x ] || [ \"${{RUBYOPT+x}}\" = x ] || \
            [ \"${{PERL5OPT+x}}\" = x ] || [ \"${{BASH_ENV+x}}\" = x ] || \
            [ \"${{ENV+x}}\" = x ] || [ \"${{ZDOTDIR+x}}\" = x ] || \
            [ \"${{GIT_CONFIG_GLOBAL+x}}\" = x ] || [ \"${{RUSTC_WRAPPER+x}}\" = x ]; then \
           printf 'language injection variable survived' > {environment_leak}; exit 91; \
         fi\n\
         if [ \"$HOME\" != \"$PWD\" ] || [ \"$TMPDIR\" != \"$PWD\" ] || \
            [ \"$XDG_CACHE_HOME\" != \"$PWD\" ] || [ \"$XDG_CONFIG_HOME\" != \"$PWD\" ] || \
            [ \"$XDG_DATA_HOME\" != \"$PWD\" ] || [ \"$XDG_STATE_HOME\" != \"$PWD\" ]; then \
           printf 'workspace environment escaped' > {environment_leak}; exit 92; \
         fi\n\
         printf 'invoked\\n' >> {invocations}\n\
         printf '%s\\n' \"$@\" > {arguments}\n\
         printf '%s\\n' \"$PWD\" > {cwd}\n\
         /bin/ls -A \"$PWD\" > {entries}\n\
         /bin/cat > {stdin}\n\
         printf 'stub response'",
        environment_leak = shell_path(&environment_leak_log),
        invocations = shell_path(&invocation_log),
        arguments = shell_path(&arguments_log),
        cwd = shell_path(&cwd_log),
        entries = shell_path(&entries_log),
        stdin = shell_path(&stdin_log),
    );
    write_stub(&evaluator_bin, "claude", &body);
    let evaluator = detect_evaluator(
        &joined_path(&[
            &evaluator_bin,
            &project_bin,
            Path::new("/bin"),
            Path::new("/usr/bin"),
        ]),
        Some(&project),
    )
    .unwrap()
    .unwrap();

    let response = run_evaluator(
        &evaluator,
        "private prompt over stdin",
        test_config(STUB_DEADLINE, 1024, 1024),
    );
    assert!(
        response.is_ok(),
        "evaluator run failed with {:?}; environment leaks: {}",
        response.as_ref().err(),
        fs::read_to_string(&environment_leak_log).unwrap_or_default()
    );
    let response = response.unwrap();

    assert_eq!(response, "stub response");
    assert_eq!(fs::read_to_string(&invocation_log).unwrap(), "invoked\n");
    assert_eq!(
        fs::read_to_string(&arguments_log)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "--print",
            "--output-format",
            "text",
            "--safe-mode",
            "--no-session-persistence",
            "--permission-mode",
            "plan",
            "--max-turns",
            "1",
            "--tools",
            "",
        ]
    );
    assert_eq!(
        fs::read_to_string(&stdin_log).unwrap(),
        "private prompt over stdin"
    );
    assert_eq!(fs::read_to_string(&entries_log).unwrap(), "");
    assert!(!helper_sentinel.exists());
    assert!(!environment_leak_log.exists());
    let workspace = fs::read_to_string(&cwd_log).unwrap();
    assert_ne!(workspace.trim(), project.to_str().unwrap());
    assert!(!Path::new(workspace.trim()).exists());
}

#[test]
fn inherited_environment_filter_retains_only_claude_auth_values() {
    let variables = [
        ("ANTHROPIC_API_KEY", "allowed-api-key"),
        ("ANTHROPIC_AUTH_TOKEN", "allowed-auth-token"),
        ("CLAUDE_CODE_OAUTH_TOKEN", "allowed-oauth-token"),
        ("NODE_OPTIONS", "private-node-injection"),
        ("PYTHONPATH", "private-python-injection"),
        ("PYTHONHOME", "private-python-home"),
        ("RUBYOPT", "private-ruby-injection"),
        ("PERL5OPT", "private-perl-injection"),
        ("BASH_ENV", "private-shell-injection"),
        ("ENV", "private-shell-env"),
        ("ZDOTDIR", "private-zsh-injection"),
        ("GIT_CONFIG_GLOBAL", "private-git-config"),
        ("RUSTC_WRAPPER", "private-rust-wrapper"),
    ]
    .map(|(name, value)| (OsString::from(name), OsString::from(value)));

    let retained = retained_evaluator_auth_environment(variables);

    assert_eq!(
        retained
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ]
    );
    assert!(!format!("{retained:?}").contains("private-"));
}

#[test]
fn nonzero_exit_returns_a_typed_non_sensitive_error() {
    let test = TestDirectory::new("nonzero");
    let project = test.directory("project");
    let bin = test.directory("bin");
    let executable = write_stub(
        &bin,
        "claude",
        "cat >/dev/null\nprintf 'private evaluator output' >&2\nexit 7",
    );
    let evaluator = detect_single(&bin, &project);

    let error = run_evaluator(
        &evaluator,
        "private evaluator prompt",
        test_config(STUB_DEADLINE, 1024, 1024),
    )
    .unwrap_err();

    assert_eq!(error, EvaluatorRunError::NonZeroExit);
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("private evaluator prompt"));
    assert!(!rendered.contains("private evaluator output"));
    assert!(!rendered.contains(executable.to_str().unwrap()));
}

#[test]
fn elapsed_deadline_kills_and_reaps_the_child() {
    let test = TestDirectory::new("timeout");
    let project = test.directory("project");
    let bin = test.directory("bin");
    write_stub(&bin, "claude", "cat >/dev/null\nwhile :; do :; done");
    let evaluator = detect_single(&bin, &project);
    let started = Instant::now();

    let error = run_evaluator(
        &evaluator,
        "prompt",
        test_config(Duration::from_millis(50), 1024, 1024),
    )
    .unwrap_err();

    assert_eq!(error, EvaluatorRunError::DeadlineExceeded);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn completed_leader_cannot_leave_a_descendant_holding_evaluator_pipes() {
    let test = TestDirectory::new("descendant-pipes");
    let project = test.directory("project");
    let bin = test.directory("bin");
    let sentinel = test.file("descendant-survived");
    let descendant_pid = test.file("descendant-pid");
    let body = format!(
        "cat >/dev/null\n\
         (sleep 1; touch {sentinel}; while :; do sleep 1; done) &\n\
         printf '%s\\n' \"$!\" > {descendant_pid}\n\
         printf 'leader response'",
        sentinel = shell_path(&sentinel),
        descendant_pid = shell_path(&descendant_pid),
    );
    write_stub(&bin, "claude", &body);
    let evaluator = detect_single(&bin, &project);
    let started = Instant::now();

    let response =
        run_evaluator(&evaluator, "prompt", test_config(STUB_DEADLINE, 1024, 1024)).unwrap();

    assert_eq!(response, "leader response");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        !fs::read_to_string(descendant_pid)
            .unwrap()
            .trim()
            .is_empty()
    );
    std::thread::sleep(Duration::from_millis(1_200));
    assert!(
        !sentinel.exists(),
        "descendant escaped evaluator group kill"
    );
}

#[test]
fn a_child_that_exits_without_reading_stdin_does_not_report_stdin_write() {
    let test = TestDirectory::new("stdin-broken-pipe");
    let project = test.directory("project");
    let bin = test.directory("bin");
    // Never touches stdin, so the prompt writer's pipe read end disappears the moment this
    // process exits. The prompt is sized well past any OS pipe buffer so the writer is still
    // blocked on the full pipe when that happens, making the resulting broken pipe
    // deterministic instead of a race against how fast the child starts.
    write_stub(&bin, "claude", "printf 'stub response'");
    let evaluator = detect_single(&bin, &project);
    let prompt = "x".repeat(600_000);
    let config = EvaluatorRunConfig::new(STUB_DEADLINE, 1_048_576, 1024, 1024).unwrap();

    let response = run_evaluator(&evaluator, &prompt, config).unwrap();

    assert_eq!(response, "stub response");
}

#[test]
fn oversized_standard_output_is_drained_and_rejected() {
    let test = TestDirectory::new("oversized");
    let project = test.directory("project");
    let bin = test.directory("bin");
    write_stub(&bin, "claude", "cat >/dev/null\nprintf '0123456789abcdefX'");
    let evaluator = detect_single(&bin, &project);

    assert_eq!(
        run_evaluator(&evaluator, "prompt", test_config(STUB_DEADLINE, 16, 1024),).unwrap_err(),
        EvaluatorRunError::StdoutTooLarge
    );
}

#[test]
fn invalid_utf8_standard_output_is_rejected() {
    let test = TestDirectory::new("invalid-utf8");
    let project = test.directory("project");
    let bin = test.directory("bin");
    write_stub(&bin, "claude", "cat >/dev/null\nprintf '\\377'");
    let evaluator = detect_single(&bin, &project);

    assert_eq!(
        run_evaluator(&evaluator, "prompt", test_config(STUB_DEADLINE, 1024, 1024),).unwrap_err(),
        EvaluatorRunError::InvalidUtf8Stdout
    );
}
