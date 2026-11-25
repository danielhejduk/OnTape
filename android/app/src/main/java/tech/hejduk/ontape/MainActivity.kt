package tech.hejduk.ontape

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.tooling.preview.Preview
import ontape.core.OnTape
import ontape.core.version
import tech.hejduk.ontape.ui.theme.OnTapeTheme

import kotlin.getValue;

class MainActivity : ComponentActivity() {
    private val onTape by lazy { OnTape.open(filesDir.resolve("ontape.db").path) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val tapes = onTape.listTapes().size
        val status = onTape.status()
        setContent {
            OnTapeTheme {
                Scaffold(modifier = Modifier.fillMaxSize()) { innerPadding ->
                    Greeting(
                        name = "OnTape ${version()} (Current status ${status.state})",
                        modifier = Modifier.padding(innerPadding)
                    )
                }
            }
        }
    }
}

@Composable
fun Greeting(name: String, modifier: Modifier = Modifier) {
    Text(
        text = "Hello $name!",
        modifier = modifier
    )
}