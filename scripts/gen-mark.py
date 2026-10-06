#!/usr/bin/env python3
"""Writes kimchi's mark and app icon: a napa stalk cut square, a sharp leaf and a leaf dissolving
into ordered dither (the grain of the interface), in one colour.

    brand/mark.svg                                ink on paper
    brand/icon.svg                                the app icon (then run scripts/make-icons.sh)
    crates/kimchi-desktop/assets/icons/mark.svg   the window's copy, in currentColor
"""
import os
import sys
STEM = [(14,14),(18,10),(22,10),(22,54),(14,54)]
UP   = [(24.5,31.5),(28.5,19.5),(47,11.5),(41,26.5)]
LOW  = [(24.5,35.5),(41,40.5),(46.5,53.5),(28.5,47.5)]
def inside(p, poly):
    x,y=p; c=False; n=len(poly)
    for i in range(n):
        x1,y1=poly[i]; x2,y2=poly[(i+1)%n]
        if (y1>y)!=(y2>y) and x < (x2-x1)*(y-y1)/(y2-y1)+x1: c=not c
    return c
B=[[0,8,2,10],[12,4,14,6],[3,11,1,9],[15,7,13,5]]
def dots(cell=2.2, size=1.75):
    out=[]; (bx,by),(tx,ty)=LOW[0],LOW[2]
    j=0; y=34.0
    while y<54:
        i=0; x=23.0
        while x<49:
            c=(x+cell/2,y+cell/2)
            if inside(c,LOW):
                t=((c[0]-bx)*(tx-bx)+(c[1]-by)*(ty-by))/((tx-bx)**2+(ty-by)**2)
                level=1-0.6*max(0,min(1,t))**1.3
                if level>(B[j%4][i%4]+0.5)/16: out.append(f'<rect x="{x:.2f}" y="{y:.2f}" width="{size}" height="{size}"/>')
            x+=cell; i+=1
        y+=cell; j+=1
    return out
pts=lambda p:" ".join(f"{x},{y}" for x,y in p)
def mark(fill="currentColor"):
    return (f'<g fill="{fill}"><polygon points="{pts(STEM)}"/><polygon points="{pts(UP)}"/>'
            + "".join(dots()) + '</g>')
def icon():
    tile=("M383.41 100 L640.59 100 C722.19 100 763 100 799.79 112.14 L806.92 113.89 C854.88 131.34 892.66 169.12 910.11 217.08 C924 261 924 301.81 924 383.41 L924 640.59 C924 722.19 924 763 911.86 799.79 L910.11 806.92 C892.66 854.88 854.88 892.66 806.92 910.11 C763 924 722.19 924 640.59 924 L383.41 924 C301.81 924 261 924 224.21 911.86 L217.08 910.11 C169.12 892.66 131.34 854.88 113.89 806.92 C100 763 100 722.19 100 640.59 L100 383.41 C100 301.81 100 261 112.14 224.21 L113.89 217.08 C131.34 169.12 169.12 131.34 217.08 113.89 C261 100 301.81 100 383.41 100 Z")
    # Dithered light in the top-left corner of the tile, as on the app's page.
    corner=[]; cell=16; Bm=[[0,8,2,10],[12,4,14,6],[3,11,1,9],[15,7,13,5]]
    for j in range(0,26):
        for i in range(0,26):
            x=100+i*cell; y=100+j*cell
            d=((i/26)**2+(j/26)**2)**0.5/1.2
            level=max(0,1-d*1.5)**1.4
            if level>(Bm[j%4][i%4]+0.5)/16: corner.append(f'<rect x="{x}" y="{y}" width="{cell-5}" height="{cell-5}"/>')
    return (f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <!-- kimchi app icon: the macOS icon grid (824 px continuous-corner tile on 1024) in near black,
       a corner of dithered light like the app's page, and the mark (brand/mark.svg) in white at
       about 58 % of the tile. scripts/gen-mark.py writes this file; scripts/make-icons.sh renders
       every size from it. -->
  <defs>
    <path id="tile" d="{tile}"/>
    <clipPath id="tile-clip"><use href="#tile"/></clipPath>
  </defs>
  <use href="#tile" fill="#0b0b0b"/>
  <g clip-path="url(#tile-clip)" fill="#fff" fill-opacity="0.16">{"".join(corner)}</g>
  <use href="#tile" fill="none" stroke="#fff" stroke-opacity="0.16" stroke-width="3"/>
  <g transform="translate(512 512) scale(10.86) translate(-30.5 -32)">{mark("#fff")}</g>
</svg>''')

def write(path, text):
    with open(path, "w") as f:
        f.write(text + "\n")


if __name__ == "__main__":
    os.chdir(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    write("crates/kimchi-desktop/assets/icons/mark.svg", '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">' + mark() + '</svg>')
    write("brand/mark.svg", '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">\n  <!-- kimchi\'s mark: a napa stalk cut square, a sharp leaf and a dithered one (the grain of the\n       interface). One colour: ink on paper or paper on ink. scripts/gen-mark.py writes it. -->\n  ' + mark("#0a0a0a") + '\n</svg>')
    write("brand/icon.svg", icon())
