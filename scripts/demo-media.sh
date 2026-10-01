#!/usr/bin/env bash
# Generates the sample media used by the browser-only demo mode (`npm run ui:dev`).
set -euo pipefail
cd "$(dirname "$0")/../ui/demo"
ff() { ffmpeg -v error -y "$@"; }

ff -f lavfi -i "gradients=s=960x540:c0=0x1a0f2e:c1=0xff5a36:c2=0xf0b44c:x0=0:y0=0:x1=960:y1=540:speed=0.02:n=3,format=yuv420p" -t 6 -r 30 -c:v libx264 -crf 30 -movflags +faststart dusk.mp4
ff -f lavfi -i "mandelbrot=s=960x540:maxiter=180:rate=30,hue=h=20:s=1.4" -t 6 -c:v libx264 -crf 32 -pix_fmt yuv420p -movflags +faststart fractal.mp4
ff -f lavfi -i "cellauto=s=960x540:rule=110:rate=30,negate,colorchannelmixer=rr=0.9:gg=0.5:bb=0.35" -t 6 -c:v libx264 -crf 32 -pix_fmt yuv420p -movflags +faststart cells.mp4
ff -f lavfi -i "gradients=s=1280x720:c0=0x0e2a1a:c1=0xa6cf5e:c2=0x2a6f8f:speed=0.01:n=3" -frames:v 1 meadow.jpg
ff -f lavfi -i "gradients=s=1280x720:c0=0x2b0b10:c1=0xff7a52:c2=0xffe3cc:speed=0.01:n=3:seed=7" -frames:v 1 ember.jpg
for v in dusk fractal cells; do
  ff -ss 1 -i "$v.mp4" -frames:v 1 -vf scale=480:-2 "$v-thumb.jpg"
  ff -i "$v.mp4" -vf "fps=2,scale=-2:72,tile=12x1" -frames:v 1 "$v-strip.jpg"
done
ff -f lavfi -i "sine=f=220:d=12,volume=0.5" -f lavfi -i "anoisesrc=d=12:c=pink:a=0.2" -filter_complex "[0][1]amix=inputs=2,tremolo=f=1.5:d=0.7" bed.m4a
echo "demo media ready in ui/demo"
