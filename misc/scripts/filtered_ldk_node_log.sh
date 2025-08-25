#!/usr/bin/env bash

# Filtered log viewer for LDK Node that excludes "falling back to default fee rate" messages
# Usage: ./misc/scripts/filtered_ldk_node_log.sh [log_file_path]

LOG_FILE="$1"

# If no log file specified, use the default pattern
if [ -z "$LOG_FILE" ]; then
    LOG_FILE="$CDK_ITESTS_DIR/ldk_mint/ldk_node.log"
fi

# Wait for log file to exist, then tail it with filtering
while [ ! -f "$LOG_FILE" ]; do 
    sleep 1
done

# Tail the log file and filter out fee rate fallback messages
tail -f "$LOG_FILE" | grep -v -E "Falling back to default of 1 sat/vb|Failed to retrieve fee rate estimates"
