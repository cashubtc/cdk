#!/bin/bash
set -e

# Configuration
REPO="${GITHUB_REPOSITORY:-cashubtc/cdk}"
DAYS_BACK="${DAYS_BACK:-7}"
MEETING_LINK="https://meet.fulmo.org/cdk-dev"
OUTPUT_DIR="meetings"

# Calculate date range (last 7 days)
SINCE_DATE=$(date -d "$DAYS_BACK days ago" -u +"%Y-%m-%dT%H:%M:%SZ")
MEETING_DATE=$(date -u +"%b %d %Y 15:00 UTC")
FILE_DATE=$(date -u +"%Y-%m-%d")

echo "Generating meeting agenda for $MEETING_DATE"
echo "Fetching data since $SINCE_DATE"

# Function to format PR/issue list
format_list() {
    local items="$1"
    if [ -z "$items" ]; then
        echo "- None"
    else
        echo "$items" | while IFS=$'\t' read -r number title url; do
            echo "- [#$number]($url) - $title"
        done
    fi
}

# Fetch merged PRs
echo "Fetching merged PRs..."
MERGED_PRS=$(gh pr list \
    --repo "$REPO" \
    --state merged \
    --search "merged:>=$SINCE_DATE" \
    --json number,title,url \
    --jq '.[] | [.number, .title, .url] | @tsv' \
    2>/dev/null || echo "")

# Fetch recently active PRs (updated in last week, but not newly created)
echo "Fetching recently active PRs..."
RECENTLY_ACTIVE_PRS=$(gh pr list \
    --repo "$REPO" \
    --state open \
    --search "updated:>=$SINCE_DATE -created:>=$SINCE_DATE" \
    --json number,title,url \
    --jq '.[] | [.number, .title, .url] | @tsv' \
    2>/dev/null || echo "")

# Fetch new PRs (opened in the last week)
echo "Fetching new PRs..."
NEW_PRS=$(gh pr list \
    --repo "$REPO" \
    --state open \
    --search "created:>=$SINCE_DATE" \
    --json number,title,url \
    --jq '.[] | [.number, .title, .url] | @tsv' \
    2>/dev/null || echo "")

# Fetch new issues
echo "Fetching new issues..."
NEW_ISSUES=$(gh issue list \
    --repo "$REPO" \
    --state open \
    --search "created:>=$SINCE_DATE" \
    --json number,title,url \
    --jq '.[] | [.number, .title, .url] | @tsv' \
    2>/dev/null || echo "")

# Fetch discussion items (labeled with meeting-discussion)
echo "Fetching discussion items..."
DISCUSSION_PRS=$(gh pr list \
    --repo "$REPO" \
    --state open \
    --label "meeting-discussion" \
    --json number,title,url \
    --jq '.[] | [.number, .title, .url] | @tsv' \
    2>/dev/null || echo "")

DISCUSSION_ISSUES=$(gh issue list \
    --repo "$REPO" \
    --state open \
    --label "meeting-discussion" \
    --json number,title,url \
    --jq '.[] | [.number, .title, .url] | @tsv' \
    2>/dev/null || echo "")

# Combine discussion items (PRs and issues)
DISCUSSION_ITEMS=$(printf "%s\n%s" "$DISCUSSION_PRS" "$DISCUSSION_ISSUES" | grep -v '^$' || echo "")

# Generate markdown
AGENDA=$(cat <<EOF
# CDK Development Meeting

$MEETING_DATE

Meeting Link: $MEETING_LINK

## Merged

$(format_list "$MERGED_PRS")

## New

### Issues

$(format_list "$NEW_ISSUES")

### PRs

$(format_list "$NEW_PRS")

## Recently Active

$(format_list "$RECENTLY_ACTIVE_PRS")

## Discussion

$(format_list "$DISCUSSION_ITEMS")
EOF
)

echo "$AGENDA"

# Output to file if requested
if [ "${OUTPUT_TO_FILE:-true}" = "true" ]; then
    mkdir -p "$OUTPUT_DIR"
    OUTPUT_FILE="$OUTPUT_DIR/$FILE_DATE-agenda.md"
    echo "$AGENDA" > "$OUTPUT_FILE"
    echo "Agenda saved to $OUTPUT_FILE"
fi

# Create GitHub Discussion if requested
if [ "${CREATE_DISCUSSION:-false}" = "true" ]; then
    echo "Creating GitHub discussion..."
    DISCUSSION_TITLE="CDK Dev Meeting - $MEETING_DATE"

    # Note: gh CLI doesn't have direct discussion creation yet, so we'd need to use the API
    # For now, we'll just output instructions
    echo "To create discussion manually, use the GitHub web interface or API"
    echo "Title: $DISCUSSION_TITLE"
fi

# Output for GitHub Actions
if [ -n "$GITHUB_OUTPUT" ]; then
    echo "agenda_file=$OUTPUT_FILE" >> "$GITHUB_OUTPUT"
    echo "meeting_date=$MEETING_DATE" >> "$GITHUB_OUTPUT"
fi
