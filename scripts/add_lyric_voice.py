#!/usr/bin/env python3
"""Add an original Japanese lyric/formant vocal layer to an Aura demo WAV."""
import math, wave
from array import array
from pathlib import Path

SRC=Path("dist/aura_neon_original.wav"); OUT=Path("dist/aura_neon_original_with_vocal.wav")
SR=44100; BPM=172; BEAT=60/BPM
phrase=[("ひ",60), ("か",62), ("り",64), ("ほ",64), ("ど",62), ("け",60), ("る",62), ("よ",64), ("る",65), ("に",64), ("き",67), ("み",65), ("と",64), ("み",62), ("つ",60), ("け",62)]
vowels={"あ":"a","か":"a","さ":"a","た":"a","な":"a","は":"a","ま":"a","ら":"a","が":"a","だ":"a","ざ":"a","ぱ":"a","ば":"a","ひ":"i","き":"i","し":"i","ち":"i","に":"i","り":"i","み":"i","い":"i","ぎ":"i","じ":"i","ぴ":"i","び":"i","る":"u","く":"u","す":"u","つ":"u","ぬ":"u","ふ":"u","む":"u","ゆ":"u","う":"u","ぐ":"u","ず":"u","ぶ":"u","ぷ":"u","ほ":"o","こ":"o","そ":"o","と":"o","の":"o","も":"o","ろ":"o","よ":"o","お":"o","ご":"o","ど":"o","ぼ":"o","ぽ":"o","え":"e","け":"e","せ":"e","て":"e","ね":"e","め":"e","れ":"e","へ":"e","げ":"e","ぜ":"e","で":"e","べ":"e","ぺ":"e"}
form={"a":(800,1150),"i":(300,2500),"u":(350,1650),"e":(500,2200),"o":(500,900)}
def render():
    with wave.open(str(SRC),'rb') as f: ch=f.getnchannels(); raw=array('h',f.readframes(f.getnframes()))
    out=list(raw); frames=len(raw)//2
    for bar in range(2,48,4):
        for j,(ly,note) in enumerate(phrase):
            start=int((bar*4+j)*BEAT*SR); length=int(.82*BEAT*SR); f0=440*2**((note-69)/12)
            vowel=vowels.get(ly,"a"); f1,f2=form[vowel]
            for k in range(length):
                i=start+k
                if i>=frames: break
                x=k/SR; attack=min(1,x/.025); rel=max(0,min(1,(length/SR-x)/.08)); env=attack*rel
                carrier=math.sin(2*math.pi*f0*x)+.35*math.sin(2*math.pi*2*f0*x)+.18*math.sin(2*math.pi*3*f0*x)
                formant=.55*math.sin(2*math.pi*f1*x)+.25*math.sin(2*math.pi*f2*x)
                # Keep the lyric layer clearly audible over the dense synth bed.
                # A short consonant-like transient improves phrase articulation.
                transient = 0.10 * math.exp(-x * 85.0)
                v=.24*env*(.72*carrier+.28*formant) + transient * env
                out[2*i]=max(-32767,min(32767,out[2*i]+int(v*32767))); out[2*i+1]=max(-32767,min(32767,out[2*i+1]+int(v*32767)))
    with wave.open(str(OUT),'wb') as f:
        f.setnchannels(2); f.setsampwidth(2); f.setframerate(SR); f.writeframes(array('h',out).tobytes())
    print(OUT)
if __name__=='__main__': render()
