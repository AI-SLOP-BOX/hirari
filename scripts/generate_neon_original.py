#!/usr/bin/env python3
"""Render a wholly original fast electronic/UTAU-style demo with distinct sound design."""
import math, os, wave
from array import array

SR = 44100; BPM = 172; BEAT = 60.0 / BPM; BARS = 48; N = int(BARS * 4 * BEAT * SR)
OUT = os.environ.get("AURA_NEON_OUT", "dist/aura_neon_original.wav")

def hz(n): return 440.0 * 2 ** ((n - 69) / 12)
def at(t): return max(0, min(N - 1, int(t * SR)))
def add(buf, t, d, fn, gain, pan=0.0):
    a, b = at(t), min(N, at(t + d)); l = math.sqrt((1-pan)*.5); r = math.sqrt((1+pan)*.5)
    for i in range(a, b):
        v = fn((i-a)/SR, d) * gain
        buf[0][i] += v*l; buf[1][i] += v*r
def e(x,d,a=.004,r=.08): return min(1,x/a) if x<a else max(0,(d-x)/r) if x>d-r else 1
def saw(f,x): return 2*((f*x)%1)-1
def fm(f,x): return math.sin(2*math.pi*f*x + 2.2*math.sin(2*math.pi*f*1.997*x))
def kick(x,d): return math.sin(2*math.pi*(170*math.exp(-x*28)+42)*x)*math.exp(-x*22)
def snare(x,d): return (.8*math.sin(2*math.pi*210*x)+.7*math.sin(2*math.pi*733*x))*math.exp(-x*30)
def hat(x,d): return math.sin(2*math.pi*3911*x)*math.exp(-x*95)
def bass(n):
    f=hz(n); return lambda x,d:(.72*saw(f,x)+.28*math.sin(2*math.pi*f*.5*x))*e(x,d,.002,.06)
def arp(n):
    f=hz(n); return lambda x,d:(.65*fm(f,x)+.35*saw(f*2,x))*math.exp(-x*8)
def vocal(n):
    f=hz(n); return lambda x,d:(.55*math.sin(2*math.pi*f*x)+.25*math.sin(2*math.pi*f*2*x)+.2*saw(f*.5,x))*e(x,d,.02,.07)

def render():
    drums=[[0.0]*N for _ in range(2)]; bassb=[[0.0]*N for _ in range(2)]; synth=[[0.0]*N for _ in range(2)]; vox=[[0.0]*N for _ in range(2)]
    chords=[(57,60,64),(54,58,61),(50,55,59),(52,57,60),(55,59,62),(48,52,55)]
    roots=[33,30,26,28,31,24]; motif=[0,3,5,7,5,3,2,3]
    for bar in range(BARS):
        t0=bar*4*BEAT; c=chords[bar%len(chords)]; root=roots[bar%len(roots)]
        drop=bar in range(16,20) or bar in range(36,40)
        for b in range(4):
            bt=t0+b*BEAT
            if not drop or b in (0,2): add(drums,bt,.16,kick,.78)
            if b in (1,3) and not drop: add(drums,bt,.13,snare,.48,.04)
            for s in range(4 if bar%8>=4 else 2): add(drums,bt+(s+.5)*BEAT/2,.035,hat,.12,-.25 if s%2 else .25)
        if not drop:
            for step in range(8): add(bassb,t0+step*.5*BEAT,.28*BEAT,bass(root+(12 if step in (3,7) else 0)),.34,-.03)
            for j,n in enumerate(c): add(synth,t0,.95*4*BEAT,lambda x,d,n=n: .22*math.sin(2*math.pi*hz(n)*x)*e(x,d,.18,.3),.12,(j-1)*.2)
        for step,off in enumerate(motif): add(synth,t0+step*.5*BEAT,.22*BEAT,arp(c[0]+12+off),.16,.2 if step%2 else -.2)
        if bar>=8 and not drop:
            line=[c[0]+12,c[1]+12,c[2]+12,c[1]+12,c[0]+12,c[1]+12,c[2]+12,c[1]+12]
            for step,n in enumerate(line): add(vox,t0+step*.5*BEAT,.38*BEAT,vocal(n),.22,.08*math.sin(step))
    mix=[[0.0]*N for _ in range(2)]
    for i in range(N):
        for ch in (0,1): mix[ch][i]=drums[ch][i]+bassb[ch][i]+synth[ch][i]+vox[ch][i]
    peak=max(1e-6,max(abs(v) for c in mix for v in c)); scale=.82/peak
    os.makedirs(os.path.dirname(OUT) or '.',exist_ok=True)
    with wave.open(OUT,'wb') as f:
        f.setnchannels(2); f.setsampwidth(2); f.setframerate(SR); p=array('h')
        for i in range(N):
            for ch in (0,1): p.append(int(max(-1,min(1,math.tanh(mix[ch][i]*scale*1.1)*.8))*32767))
        f.writeframes(p.tobytes())
    print(OUT)
if __name__=='__main__': render()
