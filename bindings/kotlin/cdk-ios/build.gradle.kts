plugins {
    `java-library`
    `maven-publish`
}

/**
 * iOS targets distributed as static libraries (.a) inside a JAR.
 */
val iosTargets = mapOf(
    "ios-arm64" to "aarch64-apple-ios",
)

iosTargets.forEach { (platform, _) ->
    tasks.register<Jar>("${platform}Jar") {
        archiveBaseName.set("cdk-ios-$platform")
        from("src/main/resources") {
            include("$platform/**")
        }
    }
}

publishing {
    publications {
        iosTargets.forEach { (platform, _) ->
            create<MavenPublication>(platform) {
                groupId = project.property("GROUP") as String
                artifactId = "cdk-ios-$platform"
                version = project.property("VERSION_NAME") as String
                artifact(tasks.named("${platform}Jar"))
            }
        }
    }
}
