use cdk_common::common::CdkVersion;
use std::str::FromStr;

#[test]
fn test_cdk_version_parsing_with_suffix() {
    let version_str = "0.15.0-rc1";
    let version = CdkVersion::from_str(version_str).unwrap();
    assert_eq!(version.implementation, "cdk");
    assert_eq!(version.major, 0);
    assert_eq!(version.minor, 15);
    assert_eq!(version.patch, 0);
}

#[test]
fn test_cdk_version_parsing_standard() {
    let version_str = "0.15.0";
    let version = CdkVersion::from_str(version_str).unwrap();
    assert_eq!(version.implementation, "cdk");
    assert_eq!(version.major, 0);
    assert_eq!(version.minor, 15);
    assert_eq!(version.patch, 0);
}

#[test]
fn test_cdk_version_parsing_complex_suffix() {
    let version_str = "0.15.0-beta.1+build123";
    let version = CdkVersion::from_str(version_str).unwrap();
    assert_eq!(version.implementation, "cdk");
    assert_eq!(version.major, 0);
    assert_eq!(version.minor, 15);
    assert_eq!(version.patch, 0);
}

#[test]
fn test_cdk_version_parsing_invalid() {
    let version_str = "0.15";
    assert!(CdkVersion::from_str(version_str).is_err());

    let version_str = "0.15.a";
    assert!(CdkVersion::from_str(version_str).is_err());
}

#[test]
fn test_cdk_version_parsing_with_implementation() {
    let version_str = "nutshell/0.16.2";
    let version = CdkVersion::from_str(version_str).unwrap();
    assert_eq!(version.implementation, "nutshell");
    assert_eq!(version.major, 0);
    assert_eq!(version.minor, 16);
    assert_eq!(version.patch, 2);
}

#[test]
fn test_cdk_version_comparison_different_implementations() {
    let v1 = CdkVersion::from_str("cdk/0.15.0").unwrap();
    let v2 = CdkVersion::from_str("nutshell/0.15.0").unwrap();

    assert_eq!(v1.partial_cmp(&v2), None);
}
