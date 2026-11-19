# Usage checks cannot describe dummy sources: Crane removes their code and
# package metadata while retaining dependency and workspace declarations.
cleanCargoToml: args:
let
  manifest = cleanCargoToml args;
in
manifest
// (if manifest ? package then {
  lints.cargo.unused_dependencies = "allow";
} else { })
  // (if manifest ? workspace then {
  workspace = manifest.workspace // {
    lints.cargo.unused_workspace_package_fields = "allow";
  };
} else { })
