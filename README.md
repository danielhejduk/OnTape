# OnTape
A music player that simulates a real cassette player. Works in CLI and GUI. Supports mp3, ogg/opus, flac, wav, m4a/aac, aiff.
## Usage
### GUI
![OnTape running on Linux](images/linux.png)  
Import an album into the app and it automatically splits into cassettes, then you just play it as a normal cassette.
### CLI
- Import an album & list it
```
import ~/Music/Album
ls
```
- Playing an album
```
insert <id of the cassette>
play
```
## Building
```
cargo build --release --workspace
```
## Why does this exists?
Music has become to boring, streaming subscriptions where you find a song and you can just stream it endlessly. I am bringing back the old way but without any physical media.
## TODO
- [X] Core
- [X] CLI tool
- [X] GTK4 app (Linux)
- [ ] Mixtapes
- [ ] Android app
- [ ] MacOS/Windows support
