#!/bin/sh
set -eu

config_file=${CDK_MINTD_CONFIG_FILE:-/etc/cdk-mintd/config.toml}
work_dir=${CDK_MINTD_WORK_DIR:-/var/lib/cdk-mintd}
init_mode=${CDK_MINTD_INIT_MODE:-}

# Initialization is intentionally one-shot and requires explicit operator
# intent. Once the database contains a configuration, edit the import document
# and use `config apply` explicitly.
if ! cdk-mintd --work-dir "$work_dir" config show >/dev/null 2>&1; then
    case "$init_mode" in
        new)
            init_flag=--new-mint
            ;;
        existing)
            init_flag=--existing-mint
            ;;
        "")
            echo "cdk-mintd configuration is absent or unreadable; set CDK_MINTD_INIT_MODE=new for a new mint or CDK_MINTD_INIT_MODE=existing for an existing mint" >&2
            exit 1
            ;;
        *)
            echo "invalid CDK_MINTD_INIT_MODE '$init_mode'; expected 'new' or 'existing'" >&2
            exit 1
            ;;
    esac
    cdk-mintd --work-dir "$work_dir" config init "$init_flag" --file "$config_file"
fi

exec cdk-mintd --work-dir "$work_dir"
