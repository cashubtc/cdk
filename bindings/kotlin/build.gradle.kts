plugins {
    kotlin("jvm") version "1.9.24" apply false
    kotlin("android") version "1.9.24" apply false
    id("com.android.library") version "8.5.1" apply false
    id("com.vanniktech.maven.publish.base") version "0.34.0" apply false
}

group = property("GROUP") as String
version = property("VERSION_NAME") as String

subprojects {
    group = rootProject.group
    version = rootProject.version

    pluginManager.withPlugin("maven-publish") {
        apply(plugin = "com.vanniktech.maven.publish.base")

        configure<com.vanniktech.maven.publish.MavenPublishBaseExtension> {
            // Publish all modules as one Central Portal deployment.
            publishToMavenCentral()
            signAllPublications()
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
