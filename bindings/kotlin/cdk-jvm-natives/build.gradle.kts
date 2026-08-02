plugins {
    `java-library`
    `maven-publish`
}

// Keep all desktop native libraries in one JAR. JNA selects the matching
// platform directory at runtime, so separate Maven coordinates are unnecessary.
// The default `jar` task already packages src/main/resources as the main
// artifact — a custom jar would collide with its output file.
val nativesSourcesJar = tasks.register<Jar>("nativesSourcesJar") {
    archiveBaseName.set("cdk-jvm-natives")
    archiveClassifier.set("sources")
    from(rootProject.projectDir.resolve("rust/src"))
}

val nativesJavadocJar = tasks.register<Jar>("nativesJavadocJar") {
    archiveBaseName.set("cdk-jvm-natives")
    archiveClassifier.set("javadoc")
}

publishing {
    publications {
        create<MavenPublication>("natives") {
            groupId = project.group as String
            artifactId = "cdk-jvm-natives"
            version = project.version as String
            from(components["java"])
            artifact(nativesSourcesJar)
            artifact(nativesJavadocJar)
        }
    }
}
