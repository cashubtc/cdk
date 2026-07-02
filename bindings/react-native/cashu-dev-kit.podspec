require 'json'

package = JSON.parse(File.read(File.join(__dir__, 'package.json')))

Pod::Spec.new do |s|
  s.name         = 'cashu-dev-kit'
  s.version      = package['version']
  s.summary      = package['description']
  s.homepage     = 'https://github.com/cashubtc/cdk'
  s.license      = package['license']
  s.authors      = 'Cashu Dev Kit Contributors'
  s.source       = { git: 'https://github.com/cashubtc/cdk.git', tag: s.version.to_s }
  s.platforms    = { ios: '13.0' }

  s.source_files = [
    'cpp/**/*.{h,hpp,cpp}',
    'nitrogen/generated/ios/**/*.{h,hpp,cpp,mm,swift}',
    'nitrogen/generated/shared/**/*.{h,hpp,cpp}',
  ]

  s.dependency 'NitroModules'

  # Link against pre-built CdkNitro XCFramework
  s.vendored_frameworks = 'ios/Frameworks/CdkNitro.xcframework'
  s.pod_target_xcconfig = {
    'HEADER_SEARCH_PATHS' => [
      '"$(PODS_TARGET_SRCROOT)/cpp"',
      '"$(PODS_TARGET_SRCROOT)/nitrogen/generated/ios"',
      '"$(PODS_TARGET_SRCROOT)/nitrogen/generated/shared"',
    ].join(' '),
    'OTHER_LDFLAGS' => '-lcdk_nitro',
  }
end
