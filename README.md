# ffrenc

utility (for me) for remuxing and transcoding videos to mp4, removing audio/video if u want, and using my normal options

## usage

```bash
# basic usage
ffrenc -i input.mov

# read paths from stdin
cat files.txt | ffrenc -i -

# strip audio
ffrenc -i input.mov --no-audio

# strip video (audio only) (also broken i think)
ffrenc -i input.mov --no-video -o {SLUG}.mp3

# custom output name (can use {SLUG} to include <this>.<ext>: {SLUG}.mp4 -> input.mp4)
ffrenc -i input.mov -o output.mp4

# overwrite if exists
ffrenc -i input.mov -y

# pass extra ffmpeg args
ffrenc -i input.mov -- -vf scale=1280:720
```

## what it does

- remuxes to mp4 (h264 video, copy audio)
- uses my preferred ffmpeg settings (crf 18, ultrafast preset)
- outputs as `{filename}.renc.mp4` by default
- supports batch processing via stdin
- shows progress during encoding

## install

```bash
cargo install --path .
```

## why

got tired of typing the same ffmpeg commands over and over and trying to get progress for things that take a long time

also this used to be a bash script that sucked, its just easier in rust
