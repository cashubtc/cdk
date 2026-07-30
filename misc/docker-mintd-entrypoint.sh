#!/bin/sh
set -eu

config_file=${CDK_MINTD_CONFIG_FILE:-/etc/cdk-mintd/config.toml}
work_dir=${CDK_MINTD_WORK_DIR:-/var/lib/cdk-mintd}

# Initialization is intentionally one-shot. Once the database contains a
# configuration, edit the import document and use `config apply` explicitly.
if ! cdk-mintd --work-dir "$work_dir" config show >/dev/null 2>&1; then
    cdk-mintd --work-dir "$work_dir" config init --file "$config_file"
fi

exec cdk-mintd --work-dir "$work_dir"
