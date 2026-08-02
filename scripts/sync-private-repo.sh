#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"

DESTINATION="${CDK_PRIVATE_REPO:-}"
TARGET_BRANCH=""
DRY_RUN=0

usage() {
    cat <<'EOF'
Push the current branch to a private GitHub repository without adding or
changing a Git remote.

Usage:
  scripts/sync-private-repo.sh [options] OWNER/REPO
  CDK_PRIVATE_REPO=OWNER/REPO scripts/sync-private-repo.sh [options]

Options:
  --branch BRANCH  Destination branch (default: current local branch)
  --dry-run        Validate and show the push without changing GitHub
  -h, --help       Show this help

The destination may also be an HTTPS or SSH GitHub repository URL. The script
verifies with GitHub CLI that the repository is private and writable, then
pushes directly to its URL. It never adds a remote or changes origin.

The worktree must be clean. Commit the state you want to sync before running
this script. Pushes must be fast-forward; existing remote history is never
force-overwritten.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

normalize_destination() {
    local destination="$1"
    local repository

    case "$destination" in
        https://github.com/*)
            repository="${destination#https://github.com/}"
            ;;
        git@github.com:*)
            repository="${destination#git@github.com:}"
            ;;
        *)
            repository="$destination"
            ;;
    esac

    repository="${repository%/}"
    repository="${repository%.git}"
    [[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] \
        || die "destination must be OWNER/REPO or a credential-free github.com URL"

    printf '%s\n' "$repository"
}

while (($# > 0)); do
    case "$1" in
        --branch)
            (($# >= 2)) || die "--branch requires a branch name"
            TARGET_BRANCH="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --*)
            die "unknown option: $1"
            ;;
        *)
            [[ -z "$DESTINATION" ]] \
                || die "provide the destination once, as an argument or CDK_PRIVATE_REPO"
            DESTINATION="$1"
            shift
            ;;
    esac
done

require_command git
require_command gh
require_command awk

[[ -n "$REPO_ROOT" ]] || die "this script must be run from a Git worktree"
[[ -n "$DESTINATION" ]] \
    || die "destination required; pass OWNER/REPO or set CDK_PRIVATE_REPO"

repository="$(normalize_destination "$DESTINATION")"
local_branch="$(git -C "$REPO_ROOT" symbolic-ref --quiet --short HEAD || true)"
if [[ -z "$TARGET_BRANCH" ]]; then
    [[ -n "$local_branch" ]] \
        || die "HEAD is detached; specify the destination with --branch"
    TARGET_BRANCH="$local_branch"
fi
git check-ref-format --branch "$TARGET_BRANCH" >/dev/null \
    || die "invalid destination branch: $TARGET_BRANCH"

[[ -z "$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=normal)" ]] \
    || die "worktree is not clean; commit the state you want to sync first"

gh auth status --hostname github.com >/dev/null 2>&1 \
    || die "GitHub CLI is not authenticated; run: gh auth login --hostname github.com"

repo_info="$(
    gh repo view "$repository" \
        --json nameWithOwner,isPrivate,viewerPermission \
        --jq '[.nameWithOwner, (.isPrivate | tostring), .viewerPermission] | @tsv'
)" || die "could not inspect GitHub repository: $repository"
IFS=$'\t' read -r canonical_repository is_private viewer_permission <<< "$repo_info"

[[ "$is_private" == "true" ]] \
    || die "refusing to sync because $canonical_repository is not private"
case "$viewer_permission" in
    ADMIN|MAINTAIN|WRITE) ;;
    *) die "authenticated GitHub account cannot push to $canonical_repository" ;;
esac

local_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)"
push_url="https://github.com/$canonical_repository.git"
refspec="HEAD:refs/heads/$TARGET_BRANCH"

printf 'Repository: %s (private)\n' "$canonical_repository"
printf 'Branch:     %s\n' "$TARGET_BRANCH"
printf 'Commit:     %s\n' "$local_commit"

push_args=(
    -C "$REPO_ROOT"
    push
)
if ((DRY_RUN == 1)); then
    push_args+=(--dry-run)
fi
push_args+=("$push_url" "$refspec")
git "${push_args[@]}"

if ((DRY_RUN == 1)); then
    printf 'Dry run completed; no remote state was changed.\n'
    exit 0
fi

remote_commit="$(
    git ls-remote "$push_url" "refs/heads/$TARGET_BRANCH" | awk 'NR == 1 { print $1 }'
)"
[[ "$remote_commit" == "$local_commit" ]] \
    || die "push verification failed: destination branch does not match local HEAD"

printf 'Verified %s at %s.\n' "$TARGET_BRANCH" "$remote_commit"
