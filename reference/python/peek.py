import sys; exec(open(sys.argv[0].replace('peek.py','ccn.py')).read())
def subs(b):
    o=0; out=[]
    while o+8<=len(b):
        cid,fl,sz=struct.unpack_from('<HHI',b,o); o+=8; d=b[o:o+sz]; o+=sz
        if fl&1: d=zlib.decompress(d[8:])
        out.append((cid,d))
        if cid==0x7F7F: break
    return out
fr=[c for c in chunks if c[0]==0x3333][0]
for cid,d in subs(body(fr)): print(hex(cid),NAMES.get(cid),len(d), d[:96].hex() if cid==0x333D else '')
