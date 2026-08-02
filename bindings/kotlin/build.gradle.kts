plugins {
    kotlin("jvm") version "1.9.24" apply false
    kotlin("android") version "1.9.24" apply false
    id("com.android.library") version "8.5.1" apply false
    id("com.vanniktech.maven.publish.base") version "0.34.0" apply false
}

// JitPack builds pass -Pgroup=com.github.<owner>.<repo> and -Pversion=<tag>
// (see jitpack.yml); Maven Central releases keep the gradle.properties
// coordinates. The published POMs must use the JitPack coordinates so that
// inter-module dependencies (cdk-android -> cdk-jvm) resolve for consumers.
group = providers.gradleProperty("group").orElse(providers.gradleProperty("GROUP")).get()
version = providers.gradleProperty("version").orElse(providers.gradleProperty("VERSION_NAME")).get()

// GPG credentials only exist in the Central publish workflow. Signing must stay
// optional so `publishToMavenLocal` works on JitPack and on developer machines.
val signingConfigured = providers.gradleProperty("signingInMemoryKey").isPresent ||
    providers.gradleProperty("signing.keyId").isPresent

subprojects {
    group = rootProject.group
    version = rootProject.version

    pluginManager.withPlugin("maven-publish") {
        apply(plugin = "com.vanniktech.maven.publish.base")

        configure<com.vanniktech.maven.publish.MavenPublishBaseExtension> {
            // Publish all modules as one Central Portal deployment.
            publishToMavenCentral()
            if (signingConfigured) {
                signAllPublications()
            }
        }

        // Gradle creates four checksums for artifacts and their signatures. Central
        // only needs MD5/SHA-1 for the artifacts, so trim the redundant sidecars
        // before the Portal plugin assembles its end-of-build deployment bundle.
        tasks.withType<PublishToMavenRepository>().configureEach {
            if (name.endsWith("ToMavenCentralRepository")) {
                doLast {
                    layout.buildDirectory.dir("publishing/mavenCentral").get().asFile
                        .walkTopDown()
                        .filter { file ->
                            file.isFile && (
                                file.name.contains(".asc.") ||
                                    file.extension == "sha256" ||
                                    file.extension == "sha512"
                            )
                        }
                        .forEach { file ->
                            check(file.delete()) {
                                "Could not remove redundant checksum: $file"
                            }
                        }
                }
            }
        }

        configure<PublishingExtension> {
            publications.withType<MavenPublication> {
                pom {
                    name.set(artifactId)
                    description.set("Cashu Development Kit — Kotlin/JVM bindings")
                    url.set("https://github.com/cashubtc/cdk-kotlin")
                    licenses {
                        license {
                            name.set("MIT")
                            url.set("https://opensource.org/licenses/MIT")
                        }
                    }
                    developers {
                        developer {
                            id.set("cashubtc")
                            name.set("Cashu BTC")
                        }
                    }
                    scm {
                        url.set("https://github.com/cashubtc/cdk-kotlin")
                        connection.set("scm:git:git://github.com/cashubtc/cdk-kotlin.git")
                        developerConnection.set("scm:git:ssh://github.com/cashubtc/cdk-kotlin.git")
                    }
                }
            }
        }
    }
}
