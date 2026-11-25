import org.jetbrains.gradle.ext.settings
import org.jetbrains.gradle.ext.taskTriggers

// Top-level build file where you can add configuration options common to all sub-projects/modules.
plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.compose) apply false
    alias(libs.plugins.idea.ext)
}

// Android Studio does not generate sources on sync, so without this the UniFFI
// bindings (ontape.core) stay unresolved in the editor until the first build.
idea.project.settings.taskTriggers {
    afterSync(":app:uniffiBindings")
}
