#!/usr/bin/env bash
# Integration tests for scripts/codexbar-tui-setup-omarchy and
# scripts/codexbar-tui-remove-omarchy.
#
# Spins up a temp HOME, stubs codexbar-tui + omarchy-launch-or-focus-tui
# on PATH, then exercises the scripts end-to-end against an assortment of
# realistic and pathological Hyprland config files. No sudo, no writes to
# the real ~/.config.
#
# Run with:  bash tests/omarchy-hotkey.sh

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SETUP="${REPO}/scripts/codexbar-tui-setup-omarchy"
REMOVE="${REPO}/scripts/codexbar-tui-remove-omarchy"

[[ -x "${SETUP}" ]]  || { echo "setup script not executable";  exit 1; }
[[ -x "${REMOVE}" ]] || { echo "remove script not executable"; exit 1; }

pass=0
fail=0
current_test=""

fail() { echo "  FAIL [${current_test}]: $*"; fail=$((fail + 1)); }
ok()   { echo "  ok   [${current_test}]: $*"; pass=$((pass + 1)); }
start() { current_test="$1"; echo; echo "${current_test}  $2"; }

# Build one-off sandbox: tmp HOME, stubbed PATH, setup+remove scripts ready.
#
# The sandbox bin is *self-contained* — the real system utilities the
# setup/remove scripts need (awk, cat, dirname, grep, mkdir, mv, rm,
# touch, bash, sh, and chmod) are symlinked in so that tests can set
# PATH="${root}/bin" exclusively without losing basic coreutils. This
# matters for T07/T08, which verify preflight behaviour when one of the
# two required binaries (codexbar-tui, omarchy-launch-or-focus-tui) is
# absent: without isolation, a globally-installed codexbar-tui in
# /usr/bin would mask the removal and make the preflight pass.
new_sandbox() {
  local root tool src
  root="$(mktemp -d)"
  mkdir -p "${root}/home/.config/hypr" "${root}/bin"
  : >"${root}/bin/codexbar-tui"
  : >"${root}/bin/omarchy-launch-or-focus-tui"
  chmod +x "${root}/bin/codexbar-tui" "${root}/bin/omarchy-launch-or-focus-tui"
  for tool in awk cat dirname grep mkdir mv rm touch bash sh chmod; do
    src="$(command -v "${tool}" 2>/dev/null || true)"
    [[ -n "${src}" ]] && ln -sf "${src}" "${root}/bin/${tool}"
  done
  echo "${root}"
}

run_setup() {
  local root="$1"
  HOME="${root}/home" \
  PATH="${root}/bin:${PATH}" \
  HYPRLAND_INSTANCE_SIGNATURE="" \
    bash "${SETUP}" >/dev/null 2>&1
}

run_remove() {
  local root="$1"
  HOME="${root}/home" \
  PATH="${root}/bin:${PATH}" \
  HYPRLAND_INSTANCE_SIGNATURE="" \
    bash "${REMOVE}" >/dev/null 2>&1
}

hash_file() { sha256sum "$1" | awk '{print $1}'; }
hash_tree() {
  # Deterministic hash of every regular file under $1 + its relative path.
  (cd "$1" && find . -type f -printf '%P\n' | LC_ALL=C sort | \
    while read -r p; do printf '%s\t%s\n' "$(sha256sum "$p" | awk '{print $1}')" "$p"; done) \
    | sha256sum | awk '{print $1}'
}
count_files() { find "$1" -type f | wc -l; }

# ---------------------------------------------------------------------------
start T01 "setup writes the expected binding + windowrule with correct escaping"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  run_setup "${root}"

  grep -qF "bindd = SUPER CTRL, U, Codex usage, exec, omarchy-launch-or-focus-tui codexbar-tui" \
    "${root}/home/.config/hypr/bindings.conf" \
    && ok "bindings.conf has exact SUPER CTRL, U line" \
    || fail "bindings.conf missing exact binding"

  # Hyprland regex: dots must be backslash-escaped in the class regex.
  grep -qF 'windowrule = tag +floating-window, match:class ^(org\.omarchy\.codexbar-tui)$' \
    "${root}/home/.config/hypr/windows.conf" \
    && ok "windows.conf has correctly-escaped windowrule" \
    || fail "windows.conf windowrule is missing or wrong"

  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T02 "setup preserves pre-existing user bindings byte-exact as a prefix"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
# existing user bindings
bindd = SUPER SHIFT, B, Browser, exec, $browser
bindd = SUPER, RETURN, Terminal, exec, $terminal --dir="$(omarchy-cmd-terminal-cwd)"

# a trailing comment the user left here
EOF
  orig_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  orig_len=$(wc -c <"${root}/home/.config/hypr/bindings.conf")

  run_setup "${root}"

  prefix_sha=$(head -c "${orig_len}" "${root}/home/.config/hypr/bindings.conf" | sha256sum | awk '{print $1}')
  [[ "${orig_sha}" == "${prefix_sha}" ]] \
    && ok "original bytes intact as a prefix" \
    || fail "user bindings were mutated in place"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T03 "5× setup reruns produce zero drift and exactly one managed block"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  run_setup "${root}"
  after_one=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  for _ in 2 3 4 5; do run_setup "${root}"; done
  after_five=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  [[ "${after_one}" == "${after_five}" ]] \
    && ok "bindings.conf byte-identical after 5 runs" || fail "drift after 5 runs"

  n=$(grep -cF "codexbar-tui-managed" "${root}/home/.config/hypr/bindings.conf")
  [[ "${n}" -eq 1 ]] && ok "one marker, not ${n}" || fail "found ${n} markers"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T04 "setup → remove returns bindings.conf to byte-exact original"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
# user prelude
bindd = SUPER SHIFT, B, Browser, exec, $browser

# another block
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  orig_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")

  run_setup  "${root}"
  run_remove "${root}"

  after_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  [[ "${orig_sha}" == "${after_sha}" ]] \
    && ok "setup→remove round-trip byte-exact" \
    || { fail "round-trip drift"; diff <(cat "${root}/home/.config/hypr/bindings.conf") - <<'EOF'
# user prelude
bindd = SUPER SHIFT, B, Browser, exec, $browser

# another block
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
    }
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T05 "remove on fresh config is a no-op and does not create files"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  orig_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")

  run_remove "${root}"

  [[ "$(hash_file "${root}/home/.config/hypr/bindings.conf")" == "${orig_sha}" ]] \
    && ok "remove left clean bindings.conf untouched" \
    || fail "remove mutated a clean bindings.conf"

  rm -f "${root}/home/.config/hypr/windows.conf"
  run_remove "${root}"
  [[ ! -f "${root}/home/.config/hypr/windows.conf" ]] \
    && ok "remove did not create windows.conf from nothing" \
    || fail "remove created windows.conf spuriously"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T06 "10× remove after setup is fully idempotent"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  run_setup  "${root}"
  run_remove "${root}"
  once=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  for _ in 1 2 3 4 5 6 7 8 9; do run_remove "${root}"; done
  after=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  [[ "${once}" == "${after}" ]] \
    && ok "10 removes produce identical file" \
    || fail "remove not idempotent"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T07 "setup refuses to run without codexbar-tui on PATH"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  rm "${root}/bin/codexbar-tui"
  # PATH is intentionally *only* the sandbox bin — it already contains
  # symlinks to awk/grep/mkdir/etc., so the setup script has every tool
  # it needs AND cannot reach a globally-installed codexbar-tui.
  if HOME="${root}/home" PATH="${root}/bin" \
     HYPRLAND_INSTANCE_SIGNATURE="" \
     bash "${SETUP}" >/dev/null 2>&1
  then
    fail "setup exited 0 despite missing codexbar-tui"
  else
    ok "setup exited nonzero"
  fi
  [[ ! -s "${root}/home/.config/hypr/bindings.conf" ]] \
    && ok "no bindings written on preflight fail" \
    || fail "bindings.conf touched on preflight fail"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T08 "setup refuses without omarchy-launch-or-focus-tui"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  rm "${root}/bin/omarchy-launch-or-focus-tui"
  # Sandbox-only PATH; see T07 for why we don't fall back to /usr/bin.
  if HOME="${root}/home" PATH="${root}/bin" \
     HYPRLAND_INSTANCE_SIGNATURE="" \
     bash "${SETUP}" >/dev/null 2>&1
  then fail "setup exited 0 without omarchy-launch-or-focus-tui"
  else ok "setup exited nonzero"
  fi
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T09 "managed block has exactly one blank-line separator (no drift)"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  for _ in 1 2 3 4 5 6 7 8 9 10; do run_setup "${root}"; done
  blanks=$(grep -c '^$' "${root}/home/.config/hypr/bindings.conf")
  [[ "${blanks}" -eq 1 ]] \
    && ok "exactly one blank line after 10 runs" \
    || fail "blank count drifted to ${blanks}"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T10 "setup handles file with no trailing newline without data loss"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  # Write "foo" with NO trailing newline.
  printf 'bindd = SUPER, RETURN, Terminal, exec, $terminal' \
    >"${root}/home/.config/hypr/bindings.conf"
  [[ $(tail -c1 "${root}/home/.config/hypr/bindings.conf" | od -c | head -1 | awk '{print $2}') != "\\n" ]] \
    && ok "test setup: file has no trailing newline" \
    || fail "precondition failed: file does end with newline"

  run_setup "${root}"

  grep -qF "bindd = SUPER, RETURN, Terminal, exec, \$terminal" \
    "${root}/home/.config/hypr/bindings.conf" \
    && ok "original terminal binding still present" \
    || fail "user's binding lost"

  grep -qF "SUPER CTRL, U, Codex usage" "${root}/home/.config/hypr/bindings.conf" \
    && ok "managed binding appended" \
    || fail "managed binding missing"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T11 "setup creates missing ~/.config/hypr directory"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  rm -rf "${root}/home/.config/hypr"
  run_setup "${root}"
  [[ -f "${root}/home/.config/hypr/bindings.conf" ]] \
    && [[ -f "${root}/home/.config/hypr/windows.conf" ]] \
    && ok "hypr dir + both files created" \
    || fail "setup did not materialize hypr dir/files"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T12 "multiple intentional blank lines in user content are preserved"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  printf '# section one\nbindd = SUPER SHIFT, B, Browser, exec, $browser\n\n\n# section two\nbindd = SUPER, RETURN, Terminal, exec, $terminal\n' \
    >"${root}/home/.config/hypr/bindings.conf"
  orig_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")

  run_setup  "${root}"
  run_remove "${root}"

  [[ "$(hash_file "${root}/home/.config/hypr/bindings.conf")" == "${orig_sha}" ]] \
    && ok "consecutive blank lines survived round-trip" \
    || fail "multi-blank-line user content was rewritten"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T13 "marker-string-in-user-content is NOT treated as our marker (exact line match only)"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  # User writes a comment that CONTAINS our marker as a substring.
  # Our awk compares whole lines with `==`, so this must not match.
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
# see also: codexbar-tui-managed (do not edit this line manually) -- remove to revert
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  orig_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")

  run_remove "${root}"
  [[ "$(hash_file "${root}/home/.config/hypr/bindings.conf")" == "${orig_sha}" ]] \
    && ok "user comment containing marker as substring untouched" \
    || fail "remove treated a substring as our marker and deleted content"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T14 "scripts touch ONLY bindings.conf + windows.conf inside ~/.config/hypr"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  # Seed the HOME with a bunch of unrelated files that must be untouched.
  mkdir -p "${root}/home/.config/hypr/apps" \
           "${root}/home/.config/other" \
           "${root}/home/.ssh" \
           "${root}/home/Documents"
  echo "do not touch" >"${root}/home/.config/hypr/hyprland.conf"
  echo "do not touch" >"${root}/home/.config/hypr/autostart.conf"
  echo "do not touch" >"${root}/home/.config/hypr/windowrules.conf"
  echo "do not touch" >"${root}/home/.config/hypr/apps/foo.conf"
  echo "do not touch" >"${root}/home/.config/other/thing.conf"
  echo "do not touch" >"${root}/home/.ssh/id_ed25519"
  echo "do not touch" >"${root}/home/Documents/notes.md"

  # Snapshot everything except the two files we *expect* to change.
  snapshot_unrelated() {
    (cd "${root}/home" && find . -type f \
      ! -path './.config/hypr/bindings.conf' \
      ! -path './.config/hypr/windows.conf' \
      -printf '%P\n' | LC_ALL=C sort | while read -r p; do
        printf '%s\t%s\n' "$(sha256sum "$p" | awk '{print $1}')" "$p"
      done) | sha256sum | awk '{print $1}'
  }

  before=$(snapshot_unrelated)
  run_setup "${root}"
  after_setup=$(snapshot_unrelated)
  run_remove "${root}"
  after_remove=$(snapshot_unrelated)

  [[ "${before}" == "${after_setup}" ]] \
    && ok "setup did not touch any unrelated files" \
    || fail "setup mutated files outside the whitelist"
  [[ "${before}" == "${after_remove}" ]] \
    && ok "remove did not touch any unrelated files" \
    || fail "remove mutated files outside the whitelist"

  # Also: no .tmp leftovers in hypr/
  shopt -s nullglob
  leftovers=("${root}/home/.config/hypr"/*.tmp "${root}/home/.config/hypr"/.*.tmp)
  shopt -u nullglob
  [[ "${#leftovers[@]}" -eq 0 ]] \
    && ok "no .tmp files left behind" \
    || fail "leftover temp files: ${leftovers[*]}"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T15 "UTF-8 content (comments, usernames, emoji) survives round-trip"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
# ノート: キー配置を変更した — 日本語コメント
# ⚡ ñoño café — user 'zeus' updated
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  orig_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")

  run_setup  "${root}"
  run_remove "${root}"

  [[ "$(hash_file "${root}/home/.config/hypr/bindings.conf")" == "${orig_sha}" ]] \
    && ok "UTF-8 content byte-identical after round-trip" \
    || fail "UTF-8 content mangled"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T16 "CRLF file: setup appends cleanly, remove strips cleanly"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  printf 'bindd = SUPER, RETURN, Terminal, exec, $terminal\r\n' \
    >"${root}/home/.config/hypr/bindings.conf"
  run_setup "${root}"

  # The original \r\n line must still be there, and our appended managed
  # block must be present.
  grep -qUF $'bindd = SUPER, RETURN, Terminal, exec, $terminal\r' \
    "${root}/home/.config/hypr/bindings.conf" \
    && ok "original CRLF line preserved" \
    || fail "original CRLF line lost"
  grep -qF "SUPER CTRL, U, Codex usage" \
    "${root}/home/.config/hypr/bindings.conf" \
    && ok "managed block appended to CRLF file" \
    || fail "managed block missing after CRLF append"

  run_remove "${root}"
  grep -qF "SUPER CTRL, U, Codex usage" \
    "${root}/home/.config/hypr/bindings.conf" \
    && fail "managed block still present after remove on CRLF" \
    || ok "managed block stripped from CRLF file"
  grep -qUF $'bindd = SUPER, RETURN, Terminal, exec, $terminal\r' \
    "${root}/home/.config/hypr/bindings.conf" \
    && ok "CRLF user line preserved through round-trip" \
    || fail "user line clobbered in CRLF round-trip"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T17 "managed block pasted in the middle of file is stripped cleanly"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  # Simulate a user who relocated the managed block mid-file (or whose
  # bindings file has content AFTER it).
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
# header
bindd = SUPER, RETURN, Terminal, exec, $terminal

# codexbar-tui-managed (do not edit this line manually)
bindd = SUPER CTRL, U, Codex usage, exec, omarchy-launch-or-focus-tui codexbar-tui

# footer
bindd = SUPER SHIFT, B, Browser, exec, $browser
EOF
  expected_after_remove=$(cat <<'EOF'
# header
bindd = SUPER, RETURN, Terminal, exec, $terminal

# footer
bindd = SUPER SHIFT, B, Browser, exec, $browser
EOF
)
  run_remove "${root}"
  actual=$(cat "${root}/home/.config/hypr/bindings.conf")
  [[ "${actual}" == "${expected_after_remove}" ]] \
    && ok "mid-file managed block stripped, surrounding content preserved" \
    || { fail "mid-file strip mangled surrounding content"
         diff <(echo "${expected_after_remove}") <(echo "${actual}") || true
       }
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T18 "user hand-edited directive under our marker is still replaced on rerun"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal

# codexbar-tui-managed (do not edit this line manually)
bindd = SUPER CTRL, Z, Codex usage, exec, codexbar-tui  # user hand-edited
EOF

  run_setup "${root}"

  # After setup, the hand-edited "SUPER CTRL, Z" line must be gone and
  # replaced with the canonical SUPER CTRL, U line.
  grep -qF "SUPER CTRL, Z, Codex usage" \
    "${root}/home/.config/hypr/bindings.conf" \
    && fail "hand-edited directive survived setup rerun" \
    || ok "hand-edited directive replaced with canonical one"
  grep -qF "SUPER CTRL, U, Codex usage" \
    "${root}/home/.config/hypr/bindings.conf" \
    && ok "canonical SUPER CTRL, U line present" \
    || fail "canonical directive missing"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T19 "large realistic bindings.conf: no memory/behavior issues"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  # Generate a 500-line realistic-looking bindings file.
  {
    for i in $(seq 1 500); do
      printf 'bindd = SUPER SHIFT, F%d, Example %d, exec, echo %d\n' "$i" "$i" "$i"
    done
  } >"${root}/home/.config/hypr/bindings.conf"
  orig_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  orig_lines=$(wc -l <"${root}/home/.config/hypr/bindings.conf")

  run_setup  "${root}"
  # Line count after setup = original + 2 (blank separator + marker) + 1 (bindd)
  after_setup_lines=$(wc -l <"${root}/home/.config/hypr/bindings.conf")
  [[ "${after_setup_lines}" -eq "$((orig_lines + 3))" ]] \
    && ok "500-line file has original_lines + 3 after setup" \
    || fail "expected $((orig_lines + 3)) lines, got ${after_setup_lines}"

  run_remove "${root}"
  after_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  [[ "${orig_sha}" == "${after_sha}" ]] \
    && ok "500-line file byte-identical after round-trip" \
    || fail "500-line file drifted through round-trip"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T20 "setup is resilient when called from a read-only cwd"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  mkdir -p "${root}/readonly"
  chmod 555 "${root}/readonly"
  (
    cd "${root}/readonly" 2>/dev/null || exit 0
    HOME="${root}/home" \
    PATH="${root}/bin:${PATH}" \
    HYPRLAND_INSTANCE_SIGNATURE="" \
      bash "${SETUP}" >/dev/null 2>&1
  )
  grep -qF "SUPER CTRL, U, Codex usage" \
    "${root}/home/.config/hypr/bindings.conf" \
    && ok "setup worked from a read-only cwd" \
    || fail "setup failed when cwd was read-only (it should not depend on cwd)"
  chmod 755 "${root}/readonly"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T21 "read-only bindings.conf: setup fails loudly without data loss"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  orig_sha=$(hash_file "${root}/home/.config/hypr/bindings.conf")
  chmod 444 "${root}/home/.config/hypr/bindings.conf"

  HOME="${root}/home" \
  PATH="${root}/bin:${PATH}" \
  HYPRLAND_INSTANCE_SIGNATURE="" \
    bash "${SETUP}" >/dev/null 2>&1
  rc=$?

  [[ "${rc}" -ne 0 ]] \
    && ok "setup exited nonzero on read-only bindings.conf" \
    || fail "setup returned 0 despite unwritable target"

  chmod 644 "${root}/home/.config/hypr/bindings.conf"
  [[ "$(hash_file "${root}/home/.config/hypr/bindings.conf")" == "${orig_sha}" ]] \
    && ok "read-only bindings.conf unchanged after failed setup" \
    || fail "read-only file was somehow modified"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T22 "no dangerous primitives in the shell scripts (static grep)"
# ---------------------------------------------------------------------------
{
  bad_patterns=('rm[[:space:]]\+-rf[[:space:]]\+["]*\$\(HOME\|/\)' \
                'rm[[:space:]]\+-rf[[:space:]]\+/' \
                'sudo[[:space:]]' \
                'dd[[:space:]]\+if=' \
                'mkfs' \
                'chmod[[:space:]]\+777' \
                'curl[[:space:]]\+.*|[[:space:]]\+sh' \
                'wget[[:space:]]\+.*|[[:space:]]\+sh' \
                'eval[[:space:]]\+\$' )
  any_found=0
  for pat in "${bad_patterns[@]}"; do
    if grep -nE "$pat" "${SETUP}" "${REMOVE}" 2>/dev/null | grep -v '^\s*#'; then
      any_found=1
      fail "dangerous pattern matched: ${pat}"
    fi
  done
  (( any_found == 0 )) && ok "no dangerous primitives in scripts"
}

# ---------------------------------------------------------------------------
start T23 "scripts both pass shellcheck if shellcheck is available"
# ---------------------------------------------------------------------------
{
  if command -v shellcheck >/dev/null 2>&1; then
    if shellcheck -s bash "${SETUP}" "${REMOVE}" >/dev/null 2>&1; then
      ok "shellcheck passes on both scripts"
    else
      fail "shellcheck complained (run 'shellcheck ${SETUP} ${REMOVE}' for details)"
    fi
  else
    ok "shellcheck not installed — skipping (best-effort check)"
  fi
}

# ---------------------------------------------------------------------------
start T24 "concurrency smoke: 4 parallel setup runs converge to one clean block"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF

  # 4 concurrent setups (stress test the strip-then-append flow).
  pids=()
  for _ in 1 2 3 4; do
    HOME="${root}/home" \
    PATH="${root}/bin:${PATH}" \
    HYPRLAND_INSTANCE_SIGNATURE="" \
      bash "${SETUP}" >/dev/null 2>&1 &
    pids+=($!)
  done
  # Swallow failures — one or more racers can legitimately fail the mv.
  for pid in "${pids[@]}"; do wait "$pid" || true; done

  # Run once more, cleanly, to converge.
  run_setup "${root}"

  # Final state must be exactly one managed block + exactly one SUPER CTRL, U line.
  n_markers=$(grep -cF "codexbar-tui-managed" "${root}/home/.config/hypr/bindings.conf")
  n_bindd=$(grep -cF "SUPER CTRL, U, Codex usage" "${root}/home/.config/hypr/bindings.conf")
  [[ "${n_markers}" -eq 1 && "${n_bindd}" -eq 1 ]] \
    && ok "converged to exactly one managed block (markers=${n_markers}, bindd=${n_bindd})" \
    || fail "post-race state: markers=${n_markers}, bindd=${n_bindd}"

  # User's original line must still be there.
  grep -qF "bindd = SUPER, RETURN, Terminal, exec, \$terminal" \
    "${root}/home/.config/hypr/bindings.conf" \
    && ok "user's original binding survived the race" \
    || fail "user's binding was lost during concurrent runs"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T25 "Hyprland syntax sanity — managed lines parse as valid directives"
# ---------------------------------------------------------------------------
{
  root="$(new_sandbox)"
  run_setup "${root}"

  # Our bindd line: exactly 4 commas before 'exec,' i.e. the Hyprland form
  # bindd = MODS, KEY, LABEL, exec, COMMAND
  bindd_line=$(grep "^bindd = SUPER CTRL, U," "${root}/home/.config/hypr/bindings.conf")
  commas=$(grep -o ',' <<<"${bindd_line}" | wc -l)
  [[ "${commas}" -ge 4 ]] \
    && ok "bindd line has ≥4 commas (well-formed bindd = MODS, KEY, LABEL, exec, CMD)" \
    || fail "bindd line malformed: '${bindd_line}'"

  # windowrule: must start with 'windowrule =' and contain 'match:class'.
  wr_line=$(grep "^windowrule = " "${root}/home/.config/hypr/windows.conf")
  [[ "${wr_line}" == *"match:class"* ]] \
    && ok "windowrule line contains 'match:class'" \
    || fail "windowrule malformed: '${wr_line}'"

  # No raw tabs inside the managed lines (Hyprland tolerates them but
  # let's be strict).
  if grep -P '\t' "${root}/home/.config/hypr/bindings.conf" "${root}/home/.config/hypr/windows.conf" >/dev/null 2>&1; then
    fail "managed output contains literal tabs"
  else
    ok "no literal tabs in managed output"
  fi
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T26 "symlinked bindings.conf is preserved as a symlink after setup/remove"
# ---------------------------------------------------------------------------
# Users often manage Hyprland config via a dotfiles repo, symlinking
# ~/.config/hypr/bindings.conf → ~/dotfiles/hypr/bindings.conf. Our scripts
# MUST write through the symlink, not replace it.
{
  root="$(new_sandbox)"
  mkdir -p "${root}/dotfiles/hypr"
  cat >"${root}/dotfiles/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  rm -f "${root}/home/.config/hypr/bindings.conf"
  ln -s "${root}/dotfiles/hypr/bindings.conf" \
        "${root}/home/.config/hypr/bindings.conf"

  run_setup "${root}"

  [[ -L "${root}/home/.config/hypr/bindings.conf" ]] \
    && ok "bindings.conf is still a symlink after setup" \
    || fail "setup replaced the symlink with a regular file"

  # The real file (symlink target) must contain the managed block.
  grep -qF "SUPER CTRL, U, Codex usage" \
    "${root}/dotfiles/hypr/bindings.conf" \
    && ok "managed block written through symlink to real file" \
    || fail "symlink target was not updated"

  # Original user binding still in the real file.
  grep -qF 'bindd = SUPER, RETURN, Terminal, exec, $terminal' \
    "${root}/dotfiles/hypr/bindings.conf" \
    && ok "user's binding preserved in symlink target" \
    || fail "user's binding lost in the symlink target"

  run_remove "${root}"

  [[ -L "${root}/home/.config/hypr/bindings.conf" ]] \
    && ok "bindings.conf is still a symlink after remove" \
    || fail "remove replaced the symlink with a regular file"

  # After remove, the target file must not contain our managed block.
  grep -qF "SUPER CTRL, U, Codex usage" \
    "${root}/dotfiles/hypr/bindings.conf" \
    && fail "managed block still present in symlink target" \
    || ok "managed block stripped from symlink target"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T27 "HOME path containing spaces works end-to-end"
# ---------------------------------------------------------------------------
{
  root="$(mktemp -d)"
  home="${root}/home with spaces"
  mkdir -p "${home}/.config/hypr" "${root}/bin"
  : >"${root}/bin/codexbar-tui"
  : >"${root}/bin/omarchy-launch-or-focus-tui"
  chmod +x "${root}/bin/codexbar-tui" "${root}/bin/omarchy-launch-or-focus-tui"
  cat >"${home}/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  orig_sha=$(hash_file "${home}/.config/hypr/bindings.conf")

  HOME="${home}" PATH="${root}/bin:${PATH}" HYPRLAND_INSTANCE_SIGNATURE="" \
    bash "${SETUP}" >/dev/null 2>&1 \
    && ok "setup succeeded with spaces in HOME path" \
    || fail "setup failed with spaces in HOME"

  grep -qF "SUPER CTRL, U, Codex usage" "${home}/.config/hypr/bindings.conf" \
    && ok "managed block written to spaced-path HOME" \
    || fail "managed block missing under spaced HOME"

  HOME="${home}" PATH="${root}/bin:${PATH}" HYPRLAND_INSTANCE_SIGNATURE="" \
    bash "${REMOVE}" >/dev/null 2>&1
  [[ "$(hash_file "${home}/.config/hypr/bindings.conf")" == "${orig_sha}" ]] \
    && ok "round-trip byte-exact under spaced HOME" \
    || fail "spaced-HOME round-trip drifted"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
start T28 "existing file permissions (mode) are preserved across rewrites"
# ---------------------------------------------------------------------------
# A user might chmod their bindings.conf to 600 for privacy. We shouldn't
# silently reset it to the default (typically 644) via mv from tmp.
{
  root="$(new_sandbox)"
  cat >"${root}/home/.config/hypr/bindings.conf" <<'EOF'
bindd = SUPER, RETURN, Terminal, exec, $terminal
EOF
  chmod 600 "${root}/home/.config/hypr/bindings.conf"
  orig_mode=$(stat -c '%a' "${root}/home/.config/hypr/bindings.conf")

  run_setup "${root}"
  after_setup_mode=$(stat -c '%a' "${root}/home/.config/hypr/bindings.conf")
  [[ "${orig_mode}" == "${after_setup_mode}" ]] \
    && ok "file mode preserved through setup (${orig_mode})" \
    || fail "file mode changed: was ${orig_mode}, now ${after_setup_mode}"

  run_remove "${root}"
  after_remove_mode=$(stat -c '%a' "${root}/home/.config/hypr/bindings.conf")
  [[ "${orig_mode}" == "${after_remove_mode}" ]] \
    && ok "file mode preserved through remove (${orig_mode})" \
    || fail "file mode changed: was ${orig_mode}, now ${after_remove_mode}"
  rm -rf "${root}"
}

# ---------------------------------------------------------------------------
echo
echo "==============================="
echo "${pass} passed, ${fail} failed"
echo "==============================="

[[ "${fail}" -eq 0 ]]
