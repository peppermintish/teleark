# Synthetic media fixtures

Generated locally with FFmpeg from a two-second blue color source (160×90,
1 frame/s) or two seconds of stereo silence (48,000 Hz). No user media or
third-party sample files are included. The generated data is distributed under
the repository’s MIT OR Apache-2.0 license. Production and tests do not invoke
FFmpeg; the four fixtures are read through the Rust metadata parsers.

Reproduction commands (run from the repository root; encoding bytes can vary
with the generator version, while tested playback properties are fixed):

```sh
ffmpeg -f lavfi -i color=c=blue:s=160x90:r=1:d=2 -map_metadata -1 -c:v libx264 -pix_fmt yuv420p -fflags +bitexact -flags:v +bitexact video.mp4
ffmpeg -f lavfi -i color=c=blue:s=160x90:r=1:d=2 -map_metadata -1 -c:v libvpx-vp9 -fflags +bitexact -flags:v +bitexact video.webm
ffmpeg -f lavfi -i anullsrc=r=48000:cl=stereo -t 2 -map_metadata -1 -c:a flac -fflags +bitexact -flags:a +bitexact audio.flac
ffmpeg -f lavfi -i anullsrc=r=48000:cl=stereo -t 2 -map_metadata -1 -c:a libmp3lame -fflags +bitexact -flags:a +bitexact audio.mp3
```

Place generated files in this directory. MP3 may report encoder padding, so its
duration assertion allows 2.0–2.1 seconds. The sparse WAV test constructs its
own header and logical payload size to verify seeks without reading samples.
