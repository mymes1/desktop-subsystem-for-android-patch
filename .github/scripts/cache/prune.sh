#!/usr/bin/env bash
# Deletes the Actions caches no run restores any more, so the repository stays under GitHub's
# 10 GB cache limit. Over the limit GitHub evicts the entries read least recently, which are
# the large compiler caches of the builds that run least often (the framework's), so a
# superseded entry left in place costs a cold build later.
#
#   prune.sh superseded <prefix> <kept key>
#       Deletes every develop cache whose key starts with <prefix> except <kept key>. A job
#       calls it after saving <kept key>: its restore keys pick the newest entry of the
#       prefix, so the older ones are never read again.
#   prune.sh all
#       Keeps only the newest develop cache of each family: the key without its trailing
#       hashes and run numbers (v0-rust-...-Linux-x64-<hash>-<hash> is v0-rust-...-Linux-x64).
#       Deletes the caches of every other ref whose branch is gone, whose pull request is
#       closed, or that no run read for a day: only a run on the same ref can read them.
#
# GH_TOKEN needs actions: write (and pull-requests: read for "all"); GH_REPO or the
# checkout names the repository.
set -euo pipefail

develop=refs/heads/develop

delete() {
    local id="$1" why="$2"
    if gh cache delete "$id" >/dev/null 2>&1; then
        echo "deleted cache $id: $why"
    else
        # Another job may have deleted it first; that is the outcome wanted.
        echo "cache $id was already gone: $why"
    fi
}

list() {
    gh cache list --limit 1000 --json id,key,ref,createdAt,lastAccessedAt,sizeInBytes "$@"
}

case "${1:-}" in
    superseded)
        prefix="${2:?name the key prefix}"
        kept="${3:?name the key that was just saved}"
        [[ "$kept" == "$prefix"* ]] || { echo "$kept does not start with $prefix" >&2; exit 2; }
        caches="$(list --ref "$develop" --key "$prefix")"
        # A save that did not go through (the upload failed, or the cache refused it) only
        # warns. The entries before it are then all there is, so they stay.
        if ! jq -e --arg kept "$kept" 'any(.[]; .key == $kept)' <<<"$caches" >/dev/null; then
            echo "$kept is not in the cache, so the entries before it stay"
            exit 0
        fi
        jq -r --arg kept "$kept" '.[] | select(.key != $kept) | "\(.id)\t\(.key)"' <<<"$caches" |
            while IFS=$'\t' read -r id key; do
                delete "$id" "$key is superseded by $kept"
            done
        ;;
    all)
        caches="$(list)"
        # Develop: the newest entry of each family stays.
        jq -r --arg develop "$develop" '.[] | select(.ref == $develop) | "\(.key)\t\(.createdAt)\t\(.id)"' <<<"$caches" |
            while IFS=$'\t' read -r key created id; do
                family="$(sed -E 's/(-([0-9a-f]{6,}|[0-9]+))+$//' <<<"$key")"
                printf '%s\t%s\t%s\t%s\n' "$family" "$created" "$id" "$key"
            done |
            sort -t $'\t' -k1,1 -k2,2r |
            awk -F '\t' 'seen[$1]++ { print $3 "\t" $4 }' |
            while IFS=$'\t' read -r id key; do
                delete "$id" "$key has a newer entry in its family"
            done
        # Every other ref.
        branches="$(gh api "repos/{owner}/{repo}/branches" --paginate --jq '.[].name')"
        open="$(gh pr list --state open --limit 200 --json number --jq '.[].number')"
        jq -r --arg develop "$develop" '
            (now - 86400) as $cut
            | .[] | select(.ref != $develop)
            | "\(.id)\t\(.ref)\t\(.key)\t\((.lastAccessedAt | sub("\.[0-9]+Z$"; "Z") | fromdateiso8601) < $cut)"' <<<"$caches" |
            while IFS=$'\t' read -r id ref key idle; do
                why=""
                if [[ "$ref" == refs/heads/* ]] && ! grep -qxF -- "${ref#refs/heads/}" <<<"$branches"; then
                    why="its branch ${ref#refs/heads/} is gone"
                elif [[ "$ref" =~ ^refs/pull/([0-9]+)/ ]] && ! grep -qxF -- "${BASH_REMATCH[1]}" <<<"$open"; then
                    why="pull request ${BASH_REMATCH[1]} is closed"
                elif [[ "$idle" == true ]]; then
                    why="no run on $ref read it for a day"
                fi
                if [[ -n "$why" ]]; then
                    delete "$id" "$key ($ref): $why"
                fi
            done
        ;;
    *)
        echo "usage: prune.sh superseded <prefix> <kept key> | prune.sh all" >&2
        exit 2
        ;;
esac
