package tech.hejduk.ontape

import android.content.ContentResolver
import android.os.Build
import android.os.Bundle
import android.provider.MediaStore.Audio.Media
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.ListItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import ontape.core.OnTape
import ontape.core.ImportTrack
import tech.hejduk.ontape.ui.theme.OnTapeTheme


class MainActivity : ComponentActivity() {
    private val onTape by lazy { OnTape.open(filesDir.resolve("ontape.db").path) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            OnTapeTheme {
                var importing by remember { mutableStateOf(false) }
                BackHandler(importing) { importing = false }
                Scaffold { padding ->
                    Column(Modifier.padding(padding)) { if (importing) ImportScreen(onTape) else ShelfScreen(onTape) { importing = true } }
                }
            }
        }
    }
}

@Composable
fun ShelfScreen(onTape: OnTape, onImport: () -> Unit) {
    var tapes by remember { mutableStateOf(onTape.listTapes()) }
    Button(onClick = onImport) { Text("Import album") }
    LazyColumn {
        items(tapes, key = { it.id }) { tape ->
            ListItem(
                headlineContent = { Text(tape.title) },
                supportingContent = { Text(tape.cassette.name) },
                trailingContent = {
                    TextButton(onClick = {
                        runCatching {
                            onTape.deleteTape(tape.id)
                        }
                        tapes = onTape.listTapes()
                    }) { Text("Delete") }
                },
            )
        }
    }
}

@Composable
fun ImportScreen(onTape: OnTape) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var albums by remember { mutableStateOf(emptyList<Album>()) }
    var query by remember { mutableStateOf("") }
    var status by remember { mutableStateOf("Tap an album to record it") }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) albums = queryAlbums(ctx.contentResolver)
    }
    val audio = if (Build.VERSION.SDK_INT >= 33) android.Manifest.permission.READ_MEDIA_AUDIO else android.Manifest.permission.READ_EXTERNAL_STORAGE
    LaunchedEffect(Unit) { permission.launch(audio) }
    OutlinedTextField(query, { query = it }, Modifier.fillMaxWidth(), placeholder = { Text("Search") })
    Text(status)
    LazyColumn {
        items(albums.filter { "${it.title} ${it.artist}".contains(query, ignoreCase = true) }) { album ->
            ListItem({ Text(album.title) }, supportingContent = { Text(album.artist) },
                modifier = Modifier.clickable {
                    status = "Recording ${album.title}..."
                    scope.launch {
                        status = runCatching {
                            withContext(Dispatchers.IO) { onTape.importTracks(album.title, album.songs) }
                        }.fold({ "Recorded ${album.title}" }, { "Failed: ${it.message}" })
                    }
                })
        }
    }
}

data class Album(val title: String, val artist: String, val songs: MutableList<ImportTrack> = mutableListOf())

@Suppress("DEPRECATION")
fun queryAlbums(resolver: ContentResolver): List<Album> {
    val albums = LinkedHashMap<Long, Album>()
    val columns = arrayOf(Media.ALBUM_ID, Media.ALBUM, Media.ARTIST, Media.TITLE, Media.DATA)
    resolver.query(Media.EXTERNAL_CONTENT_URI, columns, "${Media.IS_MUSIC} != 0", null, Media.TRACK)?.use { c ->
        while (c.moveToNext()) {
            val album = albums.getOrPut(c.getLong(0)) { Album(c.getString(1).orEmpty(), c.getString(2).orEmpty()) }
            album.songs += ImportTrack(path = c.getString(4), title = c.getString(3))
        }
    }
    return albums.values.sortedBy { it.title.lowercase() }
}