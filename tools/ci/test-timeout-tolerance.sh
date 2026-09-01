#!/usr/bin/env bash
# test-timeout-tolerance.sh — run the workspace test suite once and, when
# every failure is timeout-shaped and consistent with runner starvation,
# retry each failed test individually (serially) and downgrade the
# first-pass failures to warnings. Anything else fails the build exactly
# like a plain `cargo test` would.
#
# Parsing contract (cargo test output with CARGO_TERM_COLOR=never):
#
#   * Each test target opens with a harness line:
#         Running unittests src/lib.rs (target/debug/deps/pam_daemon-…)
#         Running tests/integration.rs (target/debug/deps/integration-…)
#         Doc-tests pam-daemon
#   * A failing target prints a `failures:` header followed by one
#         ---- <test_name> stdout ----
#     block per failed test (the test's captured stdout AND stderr — this is
#     where the test-support wrapper's PAM-TIMEOUT-* classification lines
#     land), then a second `failures:` header followed by the indented list
#     of failed test names, then a `test result: FAILED.` line.
#   * After a failing target cargo prints the exact flags needed to re-run
#     that one target:
#         error: test failed, to rerun pass `-p <pkg> --lib`
#         error: test failed, to rerun pass `-p <pkg> --test <target>`
#     We attach that hint to the most recent failing target section.
#
# Eligibility (per failed test):
#   * PAM-TIMEOUT-ENGAGED in its output block  -> never retried (a defect:
#     the process had CPU and was still late).
#   * otherwise PAM-TIMEOUT-STARVED, or a known timeout-shaped panic text
#     (`Pam daemon request timed out.` / `readiness probes within` /
#     `did not reach`)                          -> retry-once eligible.
#   * anything the parser cannot fully attribute (doctest failures, a name
#     with no output block, a section with no rerun hint, compile errors)
#     -> never retried; the first pass's failure stands.
#
# A hang is deterministic, load is not: a test that fails its serial retry
# too is treated as a real hang and fails the build.
#
# Debug/CI-fixture mode: PAM_TT_PARSE_ONLY=<logfile> parses that log, prints
# the retry plan, and exits without running cargo.

set -euo pipefail

work_dir=""

setup_work_dir() {
  work_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/pam-tt.XXXXXX")"
}

# parse_log <logfile> <planfile>
# Writes one tab-separated line per failed test to <planfile>:
#   <RETRY|FAIL> <classification> <test_name> <rerun_hint> <section_label>
parse_log() {
  local log="$1" plan="$2"
  awk '
    BEGIN { OFS = "\t"; nsec = 0; nfail = 0; mode = "none"; cur = "" }

    # register a failed test the first time we see it (block or names list)
    function register(sec, name,    key) {
      key = sec SUBSEP name
      if (!(key in seen)) {
        seen[key] = 1
        nfail++
        ord_sec[nfail] = sec
        ord_name[nfail] = name
      }
      sec_has_fail[sec] = 1
      return key
    }

    # --- new test-target section -------------------------------------
    /^[ \t]+Running / || /^[ \t]+Doc-tests / {
      nsec++
      label = $0
      gsub(/^[ \t]+/, "", label)
      sub(/ \(.*\)$/, "", label)          # drop the binary path
      sec_label[nsec] = label
      sec_doc[nsec] = ($0 ~ /Doc-tests/) ? 1 : 0
      sec_hint[nsec] = ""
      mode = "none"; cur = ""
      next
    }

    # --- rerun hint: attach to the most recent failing hint-less section
    /^error: (test|doctest) failed, to rerun pass `/ {
      hint = $0
      sub(/^error: [a-z]+ failed, to rerun pass `/, "", hint)
      sub(/`.*$/, "", hint)
      for (s = nsec; s >= 1; s--) {
        if (sec_has_fail[s] && sec_hint[s] == "") { sec_hint[s] = hint; break }
      }
      mode = "none"; cur = ""
      next
    }

    # --- captured-output block of one failed test ---------------------
    /^---- .+ (stdout|stderr) ----$/ {
      name = $0
      sub(/^---- /, "", name)
      sub(/ (stdout|stderr) ----$/, "", name)
      cur = register(nsec, name)
      has_block[cur] = 1
      mode = "block"
      next
    }

    /^failures:$/    { mode = "faillist"; cur = ""; next }
    /^test result:/  { mode = "none";     cur = ""; next }

    mode == "block" {
      if (index($0, "PAM-TIMEOUT-ENGAGED"))            engaged[cur] = 1
      if (index($0, "PAM-TIMEOUT-STARVED"))            starved[cur] = 1
      if (index($0, "Pam daemon request timed out."))  shape[cur] = "daemon-request-timeout"
      if (index($0, "readiness probes within"))        shape[cur] = "readiness-probe-timeout"
      if (index($0, "did not reach"))                  shape[cur] = "wait-for-state-timeout"
      next
    }

    # names list: indented, single token, no spaces
    mode == "faillist" && /^    [^ ]+$/ {
      name = $0
      gsub(/^[ ]+/, "", name)
      register(nsec, name)
      next
    }
    mode == "faillist" && !/^$/ { mode = "none" }

    # --- verdicts ------------------------------------------------------
    END {
      for (i = 1; i <= nfail; i++) {
        s = ord_sec[i]; name = ord_name[i]; key = s SUBSEP name
        verdict = "FAIL"; class = "unclassified"
        if (engaged[key])                    class = "engaged-marker"
        else if (sec_doc[s])                 class = "doctest-failure"
        else if (!(key in has_block))        class = "no-output-block"
        else if (starved[key])             { class = "starved-marker"; verdict = "RETRY" }
        else if (key in shape)             { class = shape[key];       verdict = "RETRY" }
        if (verdict == "RETRY" && sec_hint[s] == "") {
          verdict = "FAIL"; class = class "/no-rerun-hint"
        }
        print verdict, class, name, sec_hint[s], sec_label[s]
      }
    }
  ' "$log" > "$plan"
}

# print the plan in a human-readable form (also used by PAM_TT_PARSE_ONLY)
print_plan() {
  local plan="$1"
  local verdict class name hint label
  while IFS=$'\t' read -r verdict class name hint label; do
    echo "PLAN ${verdict} [${class}] ${name} :: hint='${hint}' :: ${label}"
  done < "$plan"
}

# derive "<pkg>/<target>" from a rerun hint like "-p pam-daemon --test integration"
hint_target_id() {
  local hint="$1" pkg="?" target="?"
  local -a words
  read -r -a words <<< "$hint"
  local i
  for ((i = 0; i < ${#words[@]}; i++)); do
    case "${words[i]}" in
      -p|--package) pkg="${words[i+1]:-?}" ;;
      --lib)        target="lib" ;;
      --test|--bin) target="${words[i+1]:-?}" ;;
    esac
  done
  echo "${pkg}/${target}"
}

warn() {
  local title="$1" msg="$2"
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::warning title=${title}::${msg}"
  else
    echo "warning: ${title}: ${msg}"
  fi
}

main() {
  # Fixture/debug mode: parse a canned log and print the plan, nothing else.
  if [[ -n "${PAM_TT_PARSE_ONLY:-}" ]]; then
    setup_work_dir
    local plan="${work_dir}/plan.tsv"
    parse_log "${PAM_TT_PARSE_ONLY}" "$plan"
    if [[ ! -s "$plan" ]]; then
      echo "DECISION: no-failures-parsed"
    else
      print_plan "$plan"
      if grep -q $'^FAIL\t' "$plan"; then
        echo "DECISION: fail (at least one failure is not retry-eligible)"
      else
        echo "DECISION: retry-all"
      fi
    fi
    return 0
  fi

  setup_work_dir
  local log="${work_dir}/first-pass.log"
  local plan="${work_dir}/plan.tsv"

  # ---- first pass ---------------------------------------------------
  local first_status=0
  CARGO_TERM_COLOR=never cargo test --workspace --all-features --locked \
    2>&1 | tee "$log" || first_status=$?
  if [[ "$first_status" -eq 0 ]]; then
    return 0
  fi

  # ---- classify the failures ---------------------------------------
  parse_log "$log" "$plan"

  if [[ ! -s "$plan" ]]; then
    echo
    echo "test-timeout-tolerance: cargo test failed but no failed tests could" \
         "be parsed from its output (compile error or unexpected format)." \
         "Failing with the first pass's status." >&2
    return "$first_status"
  fi

  echo
  echo "test-timeout-tolerance: first pass failed; classification:"
  print_plan "$plan"

  if grep -q $'^FAIL\t' "$plan"; then
    echo
    echo "test-timeout-tolerance: the following failures are NOT retry-eligible" \
         "(full output is in the first-pass log above):" >&2
    grep $'^FAIL\t' "$plan" | while IFS=$'\t' read -r _ class name hint label; do
      echo "  FAIL [${class}] ${name} (${label})" >&2
    done
    return "$first_status"
  fi

  # ---- second pass: every failure is eligible; retry serially -------
  local -a downgraded_rows=()
  local verdict class name hint label
  while IFS=$'\t' read -r verdict class name hint label; do
    local -a hint_flags
    read -r -a hint_flags <<< "$hint"
    local rerun_log="${work_dir}/rerun.log"
    echo
    echo "test-timeout-tolerance: retrying ${name} (${hint}) serially..."
    local rerun_status=0
    CARGO_TERM_COLOR=never cargo test "${hint_flags[@]}" --all-features --locked \
      -- --exact "$name" < /dev/null 2>&1 | tee "$rerun_log" || rerun_status=$?
    if [[ "$rerun_status" -ne 0 ]]; then
      echo
      echo "test-timeout-tolerance: ${name} timed out on the first pass" \
           "(${class}) and failed again when rerun alone. A hang is" \
           "deterministic, load is not: treating this as a real hang." >&2
      return 1
    fi
    # Guard against a rerun that silently selected nothing (e.g. a
    # misattributed rerun hint): passing by running zero tests proves nothing.
    if grep -q '^running 0 tests' "$rerun_log"; then
      echo
      echo "test-timeout-tolerance: retry of ${name} with '${hint}' selected" \
           "no tests, so its failure cannot be verified as transient." \
           "Failing with the first pass's status." >&2
      return "$first_status"
    fi
    downgraded_rows+=("${name}"$'\t'"$(hint_target_id "$hint")"$'\t'"${class}")
  done < "$plan"

  # ---- every retry passed: downgrade to warnings --------------------
  echo
  local row test_name target_id
  for row in "${downgraded_rows[@]}"; do
    IFS=$'\t' read -r test_name target_id class <<< "$row"
    warn "Timeout downgraded" \
      "${target_id} ${test_name} timed out on the first pass (${class}) and passed on retry"
  done

  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
      echo "## Timeout downgrades"
      echo
      echo "| Test | Target | Classification |"
      echo "| --- | --- | --- |"
      for row in "${downgraded_rows[@]}"; do
        IFS=$'\t' read -r test_name target_id class <<< "$row"
        echo "| \`${test_name}\` | ${target_id} | ${class} |"
      done
      echo
      echo "${#downgraded_rows[@]} test(s) timed out on the first pass and passed on serial retry."
    } >> "$GITHUB_STEP_SUMMARY"
  fi

  echo "test-timeout-tolerance: ${#downgraded_rows[@]} timeout-shaped failure(s)" \
       "passed on serial retry; downgraded to warnings."
  return 0
}

main "$@"
