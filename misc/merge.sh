#!/usr/bin/env bash
# Usage: just merge REVISION [--pr URL] [--message TEXT] [--check COMMAND]
#                           [--publish] [--remote upstream]
# The reviewed tip must already be fetched. Publication defaults to false.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: just merge REVISION [options]

REVISION is an already-fetched commit hash, JJ change ID, or bookmark.
  --pr URL         Include the PR title and full URL (GitHub or Forgejo).
  --message TEXT   Set the message; with --pr, replace the fetched title.
  --check COMMAND  Run optional local checks before signing.
  --publish        Also push main; the server enforces its PR and branch rules.
  --remote NAME    Upstream remote (default: upstream).

GitHub metadata uses gh authentication. Public Forgejo PRs need no login;
private Forgejo PRs can use FORGEJO_TOKEN. No hosting CLI is needed without --pr.
Use --message for a sanitized summary when PR details must not enter Git history.
Preparation and publication both create a fresh merge. No push occurs by default.
Successful publication removes the temporary bookmark and workspace.
Unpublished merges and failed attempts are retained for inspection.
EOF
}
if [[ $# == 0 ]]; then
    usage >&2
    exit 2
fi
if [[ "$1" == --help || "$1" == -h ]]; then
    usage
    exit 0
fi
tip=$1
shift
message=''
check=''
publish=false
remote=upstream
pr_url=''
while [[ $# -gt 0 ]]; do
    case "$1" in
        --publish) publish=true; shift ;;
        --help|-h) usage; exit 0 ;;
        --pr|--message|--check|--remote)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            case "$1" in
                --pr) pr_url=$2 ;;
                --message) message=$2 ;;
                --check) check=$2 ;;
                --remote) remote=$2 ;;
            esac
            shift 2
            ;;
        *) echo "Unknown option: $1" >&2; exit 2 ;;
    esac
done
message_override=$message
[[ "$remote" =~ ^[a-zA-Z0-9_-]+$ ]] || { echo 'Invalid remote name.' >&2; exit 2; }
command -v jj >/dev/null
repo_root=$(jj root)

# Resolve the reviewed revision before fetching; never silently accept a new tip.
head=$(jj log --no-graph -r "$tip" -T commit_id)
[[ "$head" =~ ^[0-9a-f]{40}$ ]] || { echo 'TIP must resolve to one Git commit.' >&2; exit 2; }
if [[ -n "$pr_url" ]]; then
    helper="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/merge_pr_message.py"
    remote_url=$(jj git remote list | awk -v remote="$remote" '$1 == remote { print $2 }')
    message=$(python3 "$helper" "$pr_url" "$head" "$remote_url" "--message=$message")
elif [[ -z "$message" ]]; then
    message="Merge ${head:0:12}"
fi
jj git fetch --remote "$remote" --branch main
base=$(jj log --no-graph -r "main@$remote" -T commit_id)
if [[ -n $(jj log --no-graph -r "$head & ::$base" -T commit_id) ]]; then
    echo 'The reviewed tip is already contained in upstream main.' >&2
    exit 1
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/cdk-merge.XXXXXX")
workspace_name=${scratch##*/}
workspace="$scratch/workspace"
echo "Merge workspace: $workspace"
echo "Retained unless publication succeeds. To clean up manually, forget workspace $workspace_name, then remove $scratch."
jj workspace add --name "$workspace_name" --sparse-patterns full \
    -r "$base" -r "$head" "$workspace"
cd "$workspace"
jj describe --message="$message

Reviewed-tip: $head
Base: $base"
if [[ -n $(jj log --no-graph -r "($base..@) & conflicts()" -T commit_id) ]]; then
    echo 'The merge or a commit being introduced has conflicts. Resolve conflicts in the affected commits, review the resulting tip, and prepare a new merge before signing.' >&2
    exit 1
fi
jj diff --from "$base" --to @ --stat
before=$(jj log --no-graph -r @ -T commit_id)
if [[ -n "$check" ]]; then
    bash -c "$check"
else
    echo 'No local validation command supplied; local tests skipped.'
fi
after=$(jj log --no-graph -r @ -T commit_id)
if [[ "$before" != "$after" ]]; then
    echo 'Validation changed the merge. Review the workspace and rerun validation before signing.' >&2
    exit 1
fi

# Check for concurrent updates after potentially lengthy validation.
if [[ -n "$pr_url" ]]; then
    latest_message=$(python3 "$helper" "$pr_url" "$head" "$remote_url" "--message=$message_override")
    [[ "$latest_message" == "$message" ]] || {
        echo 'PR title changed during validation; prepare a new merge.' >&2
        exit 1
    }
fi
jj git fetch --remote "$remote" --branch main
[[ $(jj log --no-graph -r "main@$remote" -T commit_id) == "$base" ]] || {
    echo 'Upstream main advanced; prepare and validate a new merge.' >&2
    exit 1
}
echo 'Commit message to be signed:'
jj log --no-graph -r @ -T description
jj sign -r @
signed=$(jj log --no-graph -r @ -T commit_id)
bookmark="merge/${signed:0:12}"
jj bookmark create "$bookmark" -r @
jj new
printf 'Signed merge: %s\nBookmark: %s\n' "$signed" "$bookmark"
if [[ "$publish" == false ]]; then
    echo 'Nothing was pushed. Rerun with --publish to prepare, validate, sign and publish.'
    exit 0
fi

local_main=$(jj log --no-graph -r main -T commit_id)
[[ "$local_main" == "$base" ]] || {
    echo 'Local main differs from the merge base; refusing to move it.' >&2
    exit 1
}
# Server-side PR approvals, checks and signature rules remain authoritative.
# No bypass or force-push option is used.
jj bookmark set main -r "$signed"
if ! jj git push --remote "$remote" --bookmark main; then
    echo 'Push failed. The signed merge and workspace are retained; inspect the failure before retrying.' >&2
    echo 'To retry publishing the same merge after resolving the rejection, verify main still points to the signed merge, then run:' >&2
    printf '  jj git push --remote %q --bookmark main\n' "$remote" >&2
    echo 'To rerun just merge instead, first fetch and inspect main and the remote state:' >&2
    printf '  jj git fetch --remote %q --branch main\n' "$remote" >&2
    echo 'Only if the remote is still at the base and local main is still at the signed merge, restore local main with:' >&2
    printf '  jj bookmark set main --allow-backwards -r %q\n' "$base" >&2
    echo 'Then rerun your original just merge command. The retained merge bookmark preserves the previous attempt.' >&2
    exit 1
fi

# Forgetting the workspace also abandons its empty working-copy commit.
cd "$repo_root"
jj workspace forget "$workspace_name"
jj bookmark delete "$bookmark"
rm -rf -- "$scratch"
echo 'Published merge; removed temporary bookmark and workspace.'
