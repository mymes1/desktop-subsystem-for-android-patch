#!/usr/bin/env bash
# Checks scripts/compile-check.ps1 by reading it; nothing is built.
#
# Two defects must not come back. `cmd /c start /wait` does not hand the tool's exit code
# back, so a failed `cargo clippy` reported success. `Start-Process -Wait` waits for the whole
# process tree, so a compiler server left behind by the build kept the cross-worktree lock
# held forever. The script must wait on the tool alone, keep the Idle priority class and the
# four cores, stop dotnet from leaving build servers behind, and exit with the tool's code.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
script="$repo_root/scripts/compile-check.ps1"

# Comments may explain the old approach, so only code lines are checked.
code="$(sed -e 's/\r$//' -e '/^[[:space:]]*#/d' -e 's/[[:space:]]#.*$//' "$script")"

fail() {
    echo "error: scripts/compile-check.ps1: $1" >&2
    exit 1
}

grep -Eiq '(^|[^[:alnum:]])start[^[:alnum:]].*/wait' <<<"$code" &&
    fail 'it goes back to `start ... /wait`, which loses the tool'\''s exit code'
grep -Eiq '/wait([^[:alnum:]]|$)' <<<"$code" &&
    fail 'it uses `/wait`, which loses the tool'\''s exit code'
grep -Eiq -- '-Wait([^[:alnum:]]|$)' <<<"$code" &&
    fail 'it uses `-Wait`, which waits for every descendant and can hold the lock forever'
grep -Fq 'WaitForExit()' <<<"$code" ||
    fail 'it does not wait on the tool process alone with WaitForExit()'
grep -Eq 'exit \$code' <<<"$code" && grep -Fq '$process.ExitCode' <<<"$code" ||
    fail 'it does not exit with the tool'\''s own exit code'
grep -Fq "PriorityClass = 'Idle'" <<<"$code" ||
    fail 'it no longer runs the tool at the Idle priority class'
grep -Fq "GetCurrentProcess().PriorityClass = 'Idle'" <<<"$code" ||
    fail 'it does not lower its own priority first, so compilers could start at normal priority'
test "$(grep -c -- '-j 4' <<<"$code")" -eq 2 || fail 'cargo is not limited to four cores'
grep -Fq -- '-m:4' <<<"$code" || fail 'dotnet build is not limited to four cores'
grep -Fq -- '-p:UseSharedCompilation=false' <<<"$code" ||
    fail 'dotnet build may leave VBCSCompiler.exe running'
grep -Fq -- '-nodeReuse:false' <<<"$code" ||
    fail 'dotnet build may leave MSBuild nodes running'
grep -Fq 'compile-check.lock' <<<"$code" && grep -Fq "'None')" <<<"$code" ||
    fail 'the exclusive lock across worktrees is gone'

echo "ok    compile check exits with the tool's code and waits on the tool alone"
