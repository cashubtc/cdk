plugins {
    kotlin("jvm") version "1.9.24" apply false
    kotlin("android") version "1.9.24" apply false
    id("com.android.library") version "8.5.1" apply false
    id("io.github.gradle-nexus.publish-plugin") version "2.0.0"
}

group = property("GROUP") as String
version = property("VERSION_NAME") as String

nexusPublishing {
    repositories {
        sonatype {
            nexusUrl.set(uri("https://ossrh-staging-api.central.sonatype.com/service/local/"))
            snapshotRepositoryUrl.set(uri("https://central.sonatype.com/repository/maven-snapshots/"))
            username.set(providers.environmentVariable("SONATYPE_USERNAME"))
            password.set(providers.environmentVariable("SONATYPE_PASSWORD"))
        }
    }
}

subprojects {
    pluginManager.withPlugin("maven-publish") {
        apply(plugin = "signing")

        configure<SigningExtension> {
            val signingKey = providers.environmentVariable("SIGNING_KEY")
            val signingPassword = providers.environmentVariable("SIGNING_PASSWORD")
            isRequired = signingKey.isPresent
            if (signingKey.isPresent) {
                useInMemoryPgpKeys(signingKey.get(), signingPassword.getOrElse(""))
            }
            sign(extensions.getByType<PublishingExtension>().publications)
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
