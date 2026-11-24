# Candidate release qualification

This document records evidence, not a claim that the presence of pinned inputs
proves reproducibility. The new candidate workflow is opt-in until every target
has been qualified. Existing stable/nightly workflows are not migrated by this
change.

## Required before adopting the new release path

- Build all seven groups from the same committed recipe.
- Independently reproduce complete outputs, including generated sources, all
  native targets, Maven packages, and the Swift ZIP.
- Exercise the public anonymous downloader/verifier against a hosted candidate.
- Run Android consumers with minification and 16 KiB pages, Windows-native
  consumers, and Apple consumers on supported deployment versions.
- Exercise GitHub and Maven publication with test destinations/coordinates,
  including interrupted-upload recovery and immutable-version conflicts.
- Keep the current CI and release paths until their replacements pass.

## Local evidence (2026-10-20)

The following checks ran on one Linux x86_64 workstation using Nix tools. Two
fresh builds on one machine establish local repeatability, not independent
operator verification. No candidate was published and no release collection
was created.

- All 23 release-tool tests passed, together with Ruff, ShellCheck, and
  actionlint checks.
- At recipe snapshot `fe248b0926f1cd7ae096c3e2f4c9baf564ae960d`, two clean
  generated-source builds produced identical archives and group inventories.
  `generated.tar` SHA-256:
  `cec6c43d3c86f404ea7ae5a156bb0bafb5a174f589d7f301b96b95a1121941b0`.
- At the same snapshot, two clean Linux x86_64 builds produced identical Dart
  archives, Go libraries, and group inventories. Dart archive SHA-256:
  `bec3563c2edab3a011b1e8eb06ef40a1b65af10936b0fdfcc2c3b6f862642d74`.
  Go library SHA-256:
  `5c8f8f8abe31de306702c987c58175410216a36a15cb87ece5b622b9f84771bc`.
- Both Linux builds passed six packaged Dart wallet tests; the live-mint test
  was skipped. A separate consumer check against the generated Go bindings and
  built Linux library passed four wallet, concurrency, and rate-limit tests.
- At recipe snapshot `cdd2f8cc54b885e5b05312731da8a9f7616bb147`, two clean
  Windows cross-builds produced identical Dart archives, Go DLLs, and group
  inventories. Dart archive SHA-256:
  `79bee0413f20ab42b24420137645b22fcd708a3372c2b4a60411b5b1c173e201`.
  Go DLL SHA-256:
  `b951670e41b6c884755212bff9e28c3eae2d61b89b3d846e3c8b11b4fa781566`.
  This snapshot omits unused PDB references at link time: their identities
  differed in the earlier Windows builds despite matching executable code.
  Windows-native execution remains untested.
- All seven Android native libraries built from snapshot `fe248b0926f1`:
  Kotlin on four ABIs and Dart on three. All four 64-bit libraries passed the
  16 KiB ELF LOAD alignment check.
- The first Android packaging attempt exposed two missing JUnit BOM module
  checksums in a fresh Gradle cache. Their bytes were checked against Maven
  Central and the verification metadata was updated. Packaging then passed
  strict verification with the full generated Kotlin bindings and all four
  JNI libraries. Two fresh Gradle projects executed all 37 packaging tasks and
  produced identical AAR, POM, module metadata, sources JAR, and Javadoc JAR
  files. AAR SHA-256:
  `9da7f28dfabc8a200e406bee73658784b4efb77c559172caf87f72f44a95482e`.
  Both packaging runs reused the same compiled JNI libraries; this does not
  establish independent repeatability of Android native compilation. Because
  the checksum update changed the recipe, no complete Android group inventory
  or candidate was sealed from this diagnostic run.

A Linux development host cannot qualify Darwin builds or native Apple/Windows
runtime behavior. Hosted download/publication, Android device behavior, Linux
ARM64, and cross-operator verification remain required before adoption.
