import javax.inject.Inject
import org.gradle.process.ExecOperations

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// ABIs to build the Rust core for; -Pontape.abis=arm64-v8a,armeabi-v7a picks others.
val rustAbis: List<String> = providers.gradleProperty("ontape.abis")
    .getOrElse("arm64-v8a,x86_64")
    .split(',').map { it.trim() }.filter { it.isNotEmpty() }

android {
    namespace = "tech.hejduk.ontape"
    compileSdk {
        version = release(37)
    }
    ndkVersion = "29.0.14206865"

    defaultConfig {
        applicationId = "tech.hejduk.ontape"
        minSdk = 30
        targetSdk = 37
        versionCode = 1
        versionName = "1.0"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"

        // Only ship ABIs that have the Rust core, or a device would pick one
        // where JNA loads but libontape_ffi.so is missing.
        ndk { abiFilters += rustAbis }
    }

    buildTypes {
        release {
            optimization {
                enable = true
                packageScope = setOf("androidx.**", "kotlin.**", "kotlinx.**")
            }
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }
    buildFeatures {
        compose = true
    }
}

// --- Rust core (crates/ontape-ffi) ---------------------------------------
//
// Each variant cross-compiles libontape_ffi.so with cargo-ndk into its jniLibs
// and generates the Kotlin bindings (package `ontape.core`) with the crate's
// own uniffi-bindgen. Needs `cargo install cargo-ndk`, the Android Rust targets
// (`rustup target add aarch64-linux-android x86_64-linux-android ...`) and the
// NDK above.

val cargoWorkspace: Directory = rootProject.layout.projectDirectory.dir("..")

val cargoBin: String = providers.gradleProperty("cargo").orNull
    ?: File(System.getProperty("user.home"), ".cargo/bin/cargo").takeIf { it.canExecute() }?.path
    ?: "cargo"

fun rustSources() = fileTree(cargoWorkspace) {
    include("Cargo.toml", "Cargo.lock", "crates/**")
    exclude("**/target/**")
}

abstract class CargoNdkTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations
    @get:Input abstract val cargo: Property<String>
    @get:Input abstract val abis: ListProperty<String>
    @get:Input abstract val minSdk: Property<Int>
    @get:Input abstract val release: Property<Boolean>
    @get:Internal abstract val ndkDir: DirectoryProperty
    @get:Internal abstract val workspace: DirectoryProperty
    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun build() {
        val out = outDir.get().asFile
        out.deleteRecursively()
        exec.exec {
            workingDir = workspace.get().asFile
            environment("ANDROID_NDK_HOME", ndkDir.get().asFile.path)
            executable = cargo.get()
            args("ndk")
            abis.get().forEach { args("-t", it) }
            args("-P", minSdk.get().toString(), "-o", out.path)
            args("build", "-p", "ontape-ffi", "--lib")
            if (release.get()) args("--release")
        }
        // cargo-ndk copies every cdylib it built (ontape-core is one too);
        // the app only loads the FFI library.
        out.walkTopDown()
            .filter { it.isFile && it.name != "libontape_ffi.so" }
            .forEach { it.delete() }
    }
}

abstract class UniffiBindgenTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations
    @get:Input abstract val cargo: Property<String>
    @get:Internal abstract val workspace: DirectoryProperty
    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val sources: ConfigurableFileCollection
    @get:OutputDirectory abstract val outDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val root = workspace.get().asFile
        val out = outDir.get().asFile
        out.deleteRecursively()
        // Bindings come from the metadata in the library, which is the same on
        // every target, so a host build is enough.
        exec.exec {
            workingDir = root
            commandLine(cargo.get(), "build", "-p", "ontape-ffi", "--features", "bindgen")
        }
        exec.exec {
            workingDir = root
            commandLine(
                root.resolve("target/debug/uniffi-bindgen").path, "generate",
                "--library", root.resolve("target/debug/libontape_ffi.so").path,
                "--language", "kotlin", "--no-format", "--out-dir", out.path,
            )
        }
    }
}

// The bindings are the same for every variant, so one task generates them into
// a fixed directory that is registered as a plain source dir. Android Studio
// only reliably indexes the static source sets from the DSL, not directories
// added through the Variant API, so this is what makes `ontape.core` resolve
// in the editor.
val uniffiOutDir: File = layout.buildDirectory.dir("generated/uniffi/kotlin").get().asFile

val uniffiBindings = tasks.register<UniffiBindgenTask>("uniffiBindings") {
    cargo.set(cargoBin)
    workspace.set(cargoWorkspace)
    sources.from(rustSources())
    outDir.set(uniffiOutDir)
}

android.sourceSets.getByName("main").kotlin.srcDir(uniffiOutDir)

tasks.named("preBuild") { dependsOn(uniffiBindings) }

androidComponents {
    val ndkDirectory = sdkComponents.ndkDirectory
    onVariants { variant ->
        val name = variant.name.replaceFirstChar { it.uppercase() }

        val rustLibs = tasks.register<CargoNdkTask>("cargoNdk$name") {
            cargo.set(cargoBin)
            abis.set(rustAbis)
            minSdk.set(variant.minSdk.apiLevel)
            release.set(variant.buildType == "release")
            ndkDir.set(ndkDirectory)
            workspace.set(cargoWorkspace)
            sources.from(rustSources())
        }
        variant.sources.jniLibs?.addGeneratedSourceDirectory(rustLibs, CargoNdkTask::outDir)
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    // The UniFFI bindings call into libontape_ffi.so through JNA.
    implementation(libs.jna) { artifact { type = "aar" } }
    testImplementation(libs.junit)
    androidTestImplementation(platform(libs.androidx.compose.bom))
    androidTestImplementation(libs.androidx.compose.ui.test.junit4)
    androidTestImplementation(libs.androidx.espresso.core)
    androidTestImplementation(libs.androidx.junit)
    debugImplementation(libs.androidx.compose.ui.test.manifest)
    debugImplementation(libs.androidx.compose.ui.tooling)
}
