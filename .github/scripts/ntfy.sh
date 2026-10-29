#!/usr/bin/env bash
set -euo pipefail

phase=${1:?usage: ntfy.sh start|progress <percent> <message>|finish}
if [[ -z "${NTFY_TOPIC:-}" ]]; then
    echo "::warning::NTFY_TOPIC secret is not configured; skipping push notification"
    exit 0
fi

workflow=${GITHUB_WORKFLOW:-GLUE5 Actions}
if [[ -n "${PLATFORM:-}" ]]; then
    workflow+=" (${PLATFORM})"
fi
branch=${GITHUB_REF_NAME:-unknown}
short_sha=${GITHUB_SHA:0:7}
run_url="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-}/actions/runs/${GITHUB_RUN_ID:-}"

case "$phase" in
    start)
        title="${workflow}: started"
        tags=arrow_forward
        message="Started ${workflow} on ${branch} at ${short_sha}. ${run_url}"
        ;;
    progress)
        percentage=${2:-}
        if [[ ! "$percentage" =~ ^[1-9][0-9]?$ ]] || (( percentage > 99 )); then
            echo "progress percentage must be an integer from 1 to 99" >&2
            exit 2
        fi
        shift 2
        detail=${*:-Workflow milestone reached.}
        title="${workflow}: about ${percentage}%"
        tags=hourglass_flowing_sand
        message="${workflow} is about ${percentage}% complete: ${detail} ${run_url}"
        ;;
    finish)
        status=${JOB_STATUS:-unknown}
        title="${workflow}: ${status}"
        case "$status" in
            success) tags=white_check_mark ;;
            failure) tags=x ;;
            cancelled) tags=warning ;;
            *) tags=information_source ;;
        esac
        message="Finished ${workflow}: ${status} on ${branch} at ${short_sha}. ${run_url}"
        ;;
    *)
        echo "unknown notification phase: $phase" >&2
        exit 2
        ;;
esac

if ! curl --silent --show-error --fail --retry 2 --retry-delay 1 \
    -H "Title: ${title}" -H "Tags: ${tags}" \
    --data-binary "$message" --output /dev/null "https://ntfy.sh/${NTFY_TOPIC}"; then
    echo "::warning::Could not deliver the ntfy notification"
fi
