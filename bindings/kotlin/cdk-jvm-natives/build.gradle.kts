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
)

// Register a Jar task per platform that bundles only that platform's native lib,
// plus empty sources/javadoc jars required by Maven Central.
jvmTargets.forEach { (jnaPlatform, _) ->
    tasks.register<Jar>("${jnaPlatform}Jar") {
        archiveBaseName.set("cdk-jvm-$jnaPlatform")
        from("src/main/resources") {
            include("$jnaPlatform/**")
        }
    }
    tasks.register<Jar>("${jnaPlatform}SourcesJar") {
        archiveBaseName.set("cdk-jvm-$jnaPlatform")
        archiveClassifier.set("sources")
        from(rootProject.projectDir.resolve("../../crates/cdk-ffi/src"))
    }
    tasks.register<Jar>("${jnaPlatform}JavadocJar") {
        archiveBaseName.set("cdk-jvm-$jnaPlatform")
        archiveClassifier.set("javadoc")
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
                artifact(tasks.named("${jnaPlatform}SourcesJar"))
                artifact(tasks.named("${jnaPlatform}JavadocJar"))
            }
        }
    }
}
