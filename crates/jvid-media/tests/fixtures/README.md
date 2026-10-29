`jplay.mp4` is a four-second synthetic clip with 32x32 black H.264 video and
two mono AAC streams (440 Hz and 880 Hz). Its sidecar follows jplay's
`docs/sidecar.md` v1 format, with markers at 0, 1250 and 3750 ms.

Generate the media with:

```sh
ffmpeg -hide_banner -loglevel error \
  -f lavfi -i color=c=black:s=32x32:r=10:d=4 \
  -f lavfi -i sine=frequency=440:sample_rate=16000:duration=4 \
  -f lavfi -i sine=frequency=880:sample_rate=16000:duration=4 \
  -map 0:v -map 1:a -map 2:a \
  -c:v libx264 -preset ultrafast -crf 35 -pix_fmt yuv420p \
  -c:a aac -b:a 16k -metadata:s:a:0 title=Game -metadata:s:a:1 title=Mic \
  -movflags +faststart jplay.mp4
```
