plugins {
    `java-library`
    `maven-publish`
}

// Keep all desktop native libraries in one JAR. JNA selects the matching
// platform directory at runtime, so separate Maven coordinates are unnecessary.
val nativesJar = tasks.register<Jar>("nativesJar") {
    archiveBaseName.set("cdk-jvm-natives")
    from("src/main/resources")
}

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
            groupId = project.property("GROUP") as String
            artifactId = "cdk-jvm-natives"
            version = project.property("VERSION_NAME") as String
            artifact(nativesJar)
            artifact(nativesSourcesJar)
            artifact(nativesJavadocJar)
        }
    }
}
