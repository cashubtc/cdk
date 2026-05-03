plugins {
    `java-library`
    `maven-publish`
}

/**
 * Maps JNA platform identifiers to Rust target triples.
 * Only JVM-relevant desktop targets — Android/iOS are distributed separately.
 */
val jvmTargets = mapOf(
    "linux-x86-64" to "x86_64-unknown-linux-gnu",
    "linux-aarch64" to "aarch64-unknown-linux-gnu",
    "darwin-aarch64" to "aarch64-apple-darwin",
    "win32-x86-64" to "x86_64-pc-windows-msvc",
)

// Register a Jar task per platform that bundles only that platform's native lib
jvmTargets.forEach { (jnaPlatform, _) ->
    tasks.register<Jar>("${jnaPlatform}Jar") {
        archiveBaseName.set("cdk-jvm-$jnaPlatform")
        from("src/main/resources") {
            include("$jnaPlatform/**")
        }
    }
}

publishing {
    publications {
        jvmTargets.forEach { (jnaPlatform, _) ->
            create<MavenPublication>(jnaPlatform) {
                groupId = project.property("GROUP") as String
                artifactId = "cdk-jvm-$jnaPlatform"
                version = project.property("VERSION_NAME") as String
                artifact(tasks.named("${jnaPlatform}Jar"))
            }
        }
    }

}
