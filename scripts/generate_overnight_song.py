import json, struct, pathlib
PPQ=480; BPM=96; BARS=80
out=pathlib.Path("dist/aura_final_chisa");out.mkdir(exist_ok=True)
def vlq(n):
 b=[n&127];n>>=7
 while n:b.append((n&127)|128);n>>=7
 return bytes(reversed(b))
def ev(d,x):return vlq(d)+bytes(x)
def tr(data):data+=ev(0,[255,47,0]);return b"MTrk"+struct.pack(">I",len(data))+data
def nt(notes,ch):
 x=[]
 for st,d,p,v,l in notes:x += [(st,1,[144|ch,p,v],l),(st+d,0,[128|ch,p,0],None)]
 x.sort(key=lambda q:(q[0],q[1]));o=b"";last=0
 for st,_,m,l in x:
  o+=ev(st-last,m);last=st
  if l is not None:q=l.encode();o+=ev(0,[255,5,len(q)])+q
 return tr(o)
meta=ev(0,[255,81,3])+struct.pack(">I",60000000//BPM)[1:]+ev(0,[255,88,4,4,2,24,8])
ch=[(45,48,52,57),(41,45,48,53),(48,52,55,60),(43,47,50,55)]
D=[];B=[];P=[]
for bar in range(BARS):
 s=bar*4*PPQ;c=ch[bar%4]
 section=0 if bar<8 else 1 if bar<24 else 2 if bar<32 else 3 if bar<48 else 1 if bar<64 else 4 if bar<72 else 5
 if section==0:
  if bar in (4,6):D.append((s+3*PPQ,90,42,45,None))
 else:
  kick=(0,2) if section in (1,4) else (0,1.5,2,3.25)
  for bt in kick:D.append((s+int(bt*PPQ),90,36,105 if section in (3,5) else 90,None))
  for bt in (1,3):D.append((s+bt*PPQ,90,38,100,None))
  step=0.5 if section in (3,5) else 1
  for k in range(int(4/step*2)):D.append((s+int(k*step*PPQ),50,42,68 if k%2 else 78,None))
 if section!=0:
  root=c[0]-12; pats=(0,1.5,2,3) if section in (3,5) else (0,2)
  for bt in pats:B.append((s+int(bt*PPQ),int(.72*PPQ),root+(12 if section in (3,5) and bt==2 else 0),94,None))
 if section!=4:
  for p in c:P.append((s,PPQ*4-24,p+12,72 if section in (3,5) else 58,None))
# concrete narrative lyrics, 2 bars per line
lines=["しゅうでんのまどにまちがほどける","ポケットのきっぷまだあたたかい","きょうのことばをのみこんだまま","ぬれたホームへひとりおりた","あのひとつのこえをおもいだす","さよならだけがうまくいえない","それでもあさはまどをひらいて","しらないそらをそっとてらした","わすれないでといえなかった","きみのとなりでわらいたかった","とおりすぎるひかりをつかむ","このてのなかにのこすために","きのうまでのぼくをほどいて","あたらしいくつであるきだす","もしもいつかまたあえるなら","こんどはちゃんとつたえたい","しゅうでんのまどにまちがほどける","きみのとなりでわらいたかった","このてのなかにのこすために","あしたのそらをむかえにいく"]
mel=[]
contours=[[0,2,4,2,1,0,2,4,5,4,2,1,0,2,4,5],[0,1,3,5,4,2,1,3,5,7,5,4,2,1,0,2],[0,2,4,5,7,5,4,2,4,5,7,9,7,5,4,2],[0,2,4,7,9,7,5,4,7,9,11,9,7,5,4,2]]
for i,line in enumerate(lines):
 bar=8+i*3//2
 if bar>=BARS:break
 base=bar*4*PPQ + (i%2)*PPQ
 step=PPQ//2; contour=contours[0 if i<4 else 1 if i<8 else 2 if i<16 else 3]
 chars=list(line)
 for j,chv in enumerate(chars):
  pos=base+j*step
  pitch=60+contour[(j+i)%len(contour)]
  if i in (8,9,10,11,16,17,18,19):pitch+=5
  dur=step-18 if j<len(chars)-1 else PPQ+PPQ//2
  mel.append((pos,dur,pitch,112 if i>=8 else 96,chv))
midi=b"MThd"+struct.pack(">IHHH",6,1,5,PPQ)+tr(meta)+nt(D,9)+nt(B,1)+nt(P,2)+nt(mel,0)
p=out/"aura_final_chisa.mid";p.write_bytes(midi)
tracks=[]
for track_id, source in enumerate((D, B, P, mel)):
    tracks.append({"track_id": track_id, "notes": [
        {"pitch": pitch, "start_beat": round(start / PPQ, 4),
         "length_beats": round(duration / PPQ, 4), "velocity": velocity,
         "lyric": lyric or ""}
        for start, duration, pitch, velocity, lyric in source
    ]})
(out / "aura_final_chisa.midi.json").write_text(json.dumps(tracks, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(p,len(mel),BARS*4*60/BPM)
