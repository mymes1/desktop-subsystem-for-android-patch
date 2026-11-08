#!/usr/bin/env bash
# Stops what an earlier boot check of the image test left running, before the next one starts.
#
# The boot checks boot the image one after another on one runner. When a check's step timed
# out, the runner killed the test but not the service and the VM it had started, and the next
# check's service could not listen on the vsock ports the old VM still held ("vsock port 5000:
# Address already in use"). The checks stop their own programs when they fail or run out of
# time; this is for anything that outlived them all the same. The session bus and the headless
# compositor stay: the checks share them.
set -uo pipefail

# Process names as the kernel keeps them (at most 15 characters), and the boot check binaries
# cargo built (target/debug/deps/<test>-<hash>).
programs=(crosvm passt dsa-linux-hostd dsa-compositor dsa-devices dsa-gfx)
tests='/deps/(widgets|system_screens|media)-[0-9a-f]+( |$)'

left() {
    local name
    for name in "${programs[@]}"; do
        pgrep -x "$name" || true
    done
    pgrep -f "$tests" || true
}

found="$(left)"
if [[ -z "$found" ]]; then
    echo "no boot check left anything running"
    exit 0
fi
echo "stopping what an earlier boot check left running:"
# shellcheck disable=SC2086 # one process ID a word
ps -o pid=,comm=,args= -p $(echo $found | tr ' ' ',') || true
for name in "${programs[@]}"; do
    pkill -TERM -x "$name" || true
done
pkill -TERM -f "$tests" || true
for _ in $(seq 40); do
    [[ -z "$(left)" ]] && break
    sleep 0.5
done
for name in "${programs[@]}"; do
    pkill -KILL -x "$name" || true
done
pkill -KILL -f "$tests" || true
sleep 1
if [[ -n "$(left)" ]]; then
    echo "some of them are still running:" >&2
    # shellcheck disable=SC2086
    ps -o pid=,comm=,args= -p $(left | tr '\n' ',' | sed 's/,$//') >&2 || true
    exit 1
fi
echo "stopped"
