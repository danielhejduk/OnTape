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
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import ontape.core.DeckState
import ontape.core.Direction
import ontape.core.OnTape
import ontape.core.ImportTrack
import tech.hejduk.ontape.ui.theme.OnTapeTheme

class MainActivity : ComponentActivity() {
    private val onTape by lazy { OnTape.open(filesDir.resolve("ontape.db").path) }
    private val deck by lazy { Deck(onTape) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                while (true) {
                    deck.tick()
                    delay(100)
                }
            }
        }
        setContent {
            OnTapeTheme {
                var importing by remember { mutableStateOf(false) }
                BackHandler(importing) { importing = false }
                Scaffold(
                    topBar = { TopBar(importing, onImport = { importing = true }, onBack = { importing = false }) },
                    bottomBar = { DeckBar(deck) },
                ) { padding ->
                    Column(Modifier.padding(padding)) {
                        if (importing) ImportScreen(onTape) { importing = false } else ShelfScreen(onTape, deck)
                    }
                }
            }
        }
    }

    override fun onStop() {
        super.onStop()
        deck.pause()
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TopBar(importing: Boolean, onImport: () -> Unit, onBack: () -> Unit) {
    TopAppBar(
        title = { Text(if (importing) "Record an album" else "OnTape") },
        navigationIcon = {
            if (importing) IconButton(onBack) { Icon(painterResource(R.drawable.ic_back), "Back") }
        },
        actions = {
            if (!importing) IconButton(onImport) {
                Icon(painterResource(R.drawable.ic_add), "Record an album")
            }
        },
    )
}

@Composable
fun DeckBar(deck: Deck) {
    val tape = deck.status.tape ?: return
    val state = when (val s = deck.status.state) {
        DeckState.Playing -> "Playing"
        is DeckState.Winding -> if (s.direction == Direction.FORWARD) "Fast-forwarding" else "Rewinding"
        is DeckState.Cueing -> "Cueing"
        else -> "Stopped"
    }
    Surface(tonalElevation = 3.dp) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding()
                .padding(horizontal = 16.dp, vertical = 8.dp)
        ) {
            Text(deck.title.orEmpty(), style = MaterialTheme.typography.titleMedium, maxLines = 1)
            Text("$state · Side ${tape.facing} · ${time(tape.positionMs)} / ${time(tape.sideLenMs)}")
            Text(
                deck.message ?: deck.song.orEmpty(),
                style = MaterialTheme.typography.bodySmall,
                maxLines = 1
            )
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                TextButton(deck::rewind) { Text("◀◀") }
                TextButton(deck::stop) { Text("■") }
                TextButton(deck::play) { Text("▶") }
                TextButton(deck::fastForward) { Text("▶▶") }
                TextButton(deck::flip) { Text("Flip") }
                TextButton(deck::eject) { Text("Eject") }
            }
        }
    }
}

fun time(ms: ULong): String {
    val s = (ms / 1000u).toLong()
    return "%d:%02d".format(s / 60, s % 60)
}

@Composable
fun ShelfScreen(onTape: OnTape, deck: Deck) {
    val inDeck = deck.status.tape
    var deletions by remember { mutableIntStateOf(0) }
    val tapes = remember(inDeck?.id, deck.status.state, deletions) {
        runCatching { onTape.listTapes() }.getOrDefault(emptyList())
    }
    if (tapes.isEmpty()) {
        Hint("No tapes yet.\nTap + to record an album onto tape.")
        return
    }
    LazyColumn {
        if (inDeck == null) item { Hint("Tap a tape to play it.") }
        items(tapes, key = { it.id }) { tape ->
            val loaded = inDeck?.takeIf { it.id == tape.id }
            val side = loaded?.facing ?: tape.facing
            val position = loaded?.positionMs ?: tape.positionMs
            ListItem(
                headlineContent = { Text(tape.title, maxLines = 2, overflow = TextOverflow.Ellipsis) },
                supportingContent = { Text("${tape.cassette.name} · side $side · ${time(position)}") },
                trailingContent = {
                    if (loaded == null) {
                        IconButton({
                            runCatching { onTape.deleteTape(tape.id) }
                            deletions++
                        }) { Icon(painterResource(R.drawable.ic_delete), "Throw away") }
                    }
                },
                colors = if (loaded != null) {
                    ListItemDefaults.colors(containerColor = MaterialTheme.colorScheme.secondaryContainer)
                } else {
                    ListItemDefaults.colors()
                },
                modifier = Modifier.clickable { deck.load(tape.id) },
            )
        }
    }
}

@Composable
fun Hint(text: String) {
    Box(Modifier.fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) {
        Text(text, textAlign = TextAlign.Center, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
fun ImportScreen(onTape: OnTape, onDone: () -> Unit) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var albums by remember { mutableStateOf(emptyList<Album>()) }
    var query by remember { mutableStateOf("") }
    var recording by remember { mutableStateOf<String?>(null) }
    var problem by remember { mutableStateOf<String?>(null) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) albums = queryAlbums(ctx.contentResolver)
    }
    val audio = if (Build.VERSION.SDK_INT >= 33) android.Manifest.permission.READ_MEDIA_AUDIO else android.Manifest.permission.READ_EXTERNAL_STORAGE
    LaunchedEffect(Unit) { permission.launch(audio) }
    OutlinedTextField(
        query, { query = it },
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        placeholder = { Text("Search albums") },
        leadingIcon = { Icon(painterResource(R.drawable.ic_search), null) },
        singleLine = true,
    )
    recording?.let {
        LinearProgressIndicator(Modifier.fillMaxWidth().padding(horizontal = 16.dp))
        Text("Recording $it...", Modifier.padding(16.dp, 8.dp), style = MaterialTheme.typography.bodyMedium)
    }
    problem?.let {
        Text(it, Modifier.padding(16.dp, 8.dp), color = MaterialTheme.colorScheme.error)
    }
    LazyColumn {
        items(albums.filter { "${it.title} ${it.artist}".contains(query, ignoreCase = true) }) { album ->
            ListItem(
                headlineContent = { Text(album.title) },
                supportingContent = { Text("${album.artist} · ${album.songs.size} songs") },
                modifier = Modifier.clickable(enabled = recording == null) {
                    recording = album.title
                    problem = null
                    scope.launch {
                        runCatching {
                            withContext(Dispatchers.IO) { onTape.importTracks(album.title, album.songs) }
                        }.onSuccess { onDone() }.onFailure { problem = describe(it) }
                        recording = null
                    }
                },
            )
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