package tech.hejduk.ontape

import android.media.MediaPlayer
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateOf
import ontape.core.AudibleTrack
import ontape.core.DeckEvent
import ontape.core.DeckState
import ontape.core.DeckStatus
import ontape.core.Direction
import ontape.core.OnTape
import ontape.core.OnTapeException
import ontape.core.maxDriftMs
import kotlin.math.abs

class Deck(private val onTape: OnTape) {
    private val speaker = Speaker()

    var status: DeckStatus by mutableStateOf(onTape.status())
    var title: String? by mutableStateOf(null)
    var song: String? by mutableStateOf(null)
    var message: String? by mutableStateOf(null)

    private var titledTape: Long? = null

    init {
        refresh()
    }

    fun load(id: Long) = press {
        if (status.tape != null) it.eject()
        it.insert(id)
        it.play()
    }

    fun play() = press { it.play() }
    fun stop() = press { it.stop() }
    fun fastForward() = press { it.fastForward() }
    fun rewind() = press { it.rewind() }
    fun flip() = press { it.flip() }
    fun eject() = press { it.eject() }

    fun tick() {
        runCatching { onTape.tick() }
            .onSuccess { events -> events.lastOrNull()?.let { message = describe(it) } }
            .onFailure { message = describe(it) }
        refresh()
    }

    fun pause() {
        if (status.state != DeckState.Empty) runCatching { onTape.stop() }
        refresh()
        speaker.release()
    }

    private fun press(button: (OnTape) -> Unit) {
        message = runCatching { button(onTape) }.exceptionOrNull()?.let(::describe)
        refresh()
    }

    private fun refresh() {
        status = onTape.status()
        val tape = status.tape
        if (tape?.id != titledTape) {
            titledTape = tape?.id
            title = tape?.let { runCatching { onTape.tapeTitle(it.id) }.getOrNull() }
        }
        runCatching {
            val track = onTape.audible()
            song = track?.title
            speaker.follow(track)
        }.onFailure { message = describe(it) }
    }
}

private class Speaker {
    private val maxDrift = maxDriftMs().toLong()
    private var player: MediaPlayer? = null
    private var path: String? = null

    fun follow(track: AudibleTrack?) {
        if (track == null) {
            player?.takeIf { it.isPlaying }?.pause()
            return
        }
        val offset = track.offsetMs.toLong()
        val p = player?.takeIf { path == track.path } ?: load(track.path)
        if (p.isPlaying) {
            if (abs(p.currentPosition - offset) > maxDrift) p.seekTo(offset, MediaPlayer.SEEK_CLOSEST)
        } else if (offset + maxDrift < p.duration) {
            p.seekTo(offset, MediaPlayer.SEEK_CLOSEST)
            p.start()
        }
    }

    private fun load(path: String): MediaPlayer {
        release()
        val p = MediaPlayer()
        try {
            p.setDataSource(path)
            p.prepare()
        } catch (e: Exception) {
            p.release()
            throw e
        }
        player = p
        this.path = path
        return p
    }

    fun release() {
        player?.release()
        player = null
        path = null
    }
}

fun describe(event: DeckEvent): String = when (event) {
    is DeckEvent.EndOfSide -> "End of side ${event.side}. Flip the tape to carry on."
    is DeckEvent.WindFinished -> if (event.direction == Direction.FORWARD) "Fast-forwarded to the end" else "Rewound to the start"
}

fun describe(e: Throwable): String = when (e) {
    is OnTapeException.DeckEmpty -> "No tape in the deck"
    is OnTapeException.TapeInDeck -> "Eject the tape first"
    is OnTapeException.Storage -> e.reason
    is OnTapeException.Import -> e.reason
    else -> e.message?.takeIf { it.isNotBlank() } ?: e.toString()
}