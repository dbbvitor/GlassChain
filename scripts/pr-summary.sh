#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 dbbvitor
#
# Compile the GitHub Actions results for one commit into a single rolling PR
# comment. Checks that report natively (Codecov statuses, Code Scanning
# annotations) are deliberately not duplicated; everything that only paints
# check marks gets one table here, updated in place as each workflow finishes.
#
# Env: REPO=owner/repo PR_NUMBER=<n> HEAD_SHA=<sha> GH_TOKEN=<token>
#      DRY_RUN=1  print the comment instead of posting it
#
# Local dry run:
#   REPO=dbbvitor/GlassChain PR_NUMBER=186 HEAD_SHA=$(git rev-parse HEAD) \
#     DRY_RUN=1 scripts/pr-summary.sh
set -euo pipefail

repo="${REPO:?REPO is required}"
sha="${HEAD_SHA:?HEAD_SHA is required}"
marker='<!-- pr-summary -->'

if [ -z "${PR_NUMBER:-}" ]; then
    echo "no pull request associated with this run; nothing to do"
    exit 0
fi

icon() {
    case "$1" in
        success) printf '✅' ;;
        skipped) printf '⏭️' ;;
        cancelled) printf '🚫' ;;
        failure | timed_out | startup_failure | action_required | stale) printf '❌' ;;
        *) printf '⏳' ;;
    esac
}

# Every Actions run for the commit, this summary and GitHub's dynamic CodeQL
# workflows (which report natively through Code Scanning) excluded.
runs=$(gh api "repos/$repo/actions/runs?head_sha=$sha&per_page=100" --paginate \
    --jq '.workflow_runs[]
        | select(.path | startswith("dynamic/") | not)
        | select(.name != "PR summary")
        | [.name, (.conclusion // .status), (.id|tostring), .html_url] | @tsv' | sort)

body=$(mktemp)
trap 'rm -f "$body"' EXIT

{
    printf '%s\n' "$marker"
    printf '### CI summary — [`%s`](https://github.com/%s/commit/%s)\n\n' \
        "${sha:0:7}" "$repo" "$sha"
    printf '| | Workflow | Result |\n|---|---|---|\n'
    while IFS=$'\t' read -r name state id url; do
        [ -n "$name" ] || continue
        printf '| %s | [%s](%s) | %s |\n' "$(icon "$state")" "$name" "$url" "$state"
    done <<<"$runs"

    printf '\n<details><summary>Jobs</summary>\n\n'
    printf '| Workflow | Job | Result |\n|---|---|---|\n'
    while IFS=$'\t' read -r name _state id _url; do
        [ -n "$name" ] || continue
        jobs=$(gh api "repos/$repo/actions/runs/$id/jobs" --paginate \
            --jq '.jobs[] | [.name, (.conclusion // .status)] | @tsv')
        while IFS=$'\t' read -r job job_state; do
            [ -n "$job" ] || continue
            printf '| %s | %s | %s %s |\n' "$name" "$job" "$(icon "$job_state")" "$job_state"
        done <<<"$jobs"
    done <<<"$runs"
    printf '\n</details>\n\n'
    printf '_One rolling comment per PR, edited in place as each workflow completes. '
    printf 'Codecov and Code Scanning report natively and are not repeated here._\n'
} >"$body"

if [ "${DRY_RUN:-}" = "1" ]; then
    cat "$body"
    exit 0
fi

comment_id=$(gh api "repos/$repo/issues/$PR_NUMBER/comments?per_page=100" --paginate \
    --jq ".[] | select(.body | contains(\"$marker\")) | .id" | tail -n1)

method=(--method POST "repos/$repo/issues/$PR_NUMBER/comments")
if [ -n "$comment_id" ]; then
    method=(--method PATCH "repos/$repo/issues/comments/$comment_id")
fi
jq -n --rawfile body "$body" '{body: $body}' | gh api "${method[@]}" --input -
