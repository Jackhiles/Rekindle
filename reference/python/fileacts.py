import sys,re; exec(open(sys.argv[0].replace('fileacts.py','ccn.py')).read())
def subs(b):
    o=0; out=[]
    while o+8<=len(b):
        cid,fl,sz=struct.unpack_from('<HHI',b,o); o+=8; d=b[o:o+sz]; o+=sz
        if fl&1: d=zlib.decompress(d[8:])
        out.append((cid,d))
        if cid==0x7F7F: break
    return out
for fr in [c for c in chunks if c[0]==0x3333]:
    d=dict(subs(body(fr))); name=d[0x3335].split(b'\0')[0].decode(); ev=d.get(0x333D,b'')
    q=ev.find(b'ERev'); esz=struct.unpack_from('<I',ev,q+4)[0]; e=q+8; end=e+esz; gi=0
    while e<end:
        gs,nc,na=struct.unpack_from('<hBB',ev,e); p=e+14; gi+=1; rows=[]; hit=False
        for k in range(nc+na):
            cs,ot,num=struct.unpack_from('<Hhh',ev,p)
            hdr=16 if k<nc else 14; npar=ev[p+12]; pp=p+hdr; ps=[]
            for _ in range(npar):
                psz,code=struct.unpack_from('<Hh',ev,pp); ps.append((code,ev[pp+4:pp+psz])); pp+=psz
            if ot>=32 and num in (88,90,109,110,113): hit=True
            rows.append(('C' if k<nc else 'A',ot,num,ps))
            p+=cs
        if hit and (len(sys.argv) < 3 or name == sys.argv[2]):
            print('==',name,'event',gi)
            for r in rows:
                print('  ',r[0],r[1],r[2],[(c,b.hex() if len(b)<60 else b[:60].hex()+'..', re.findall(rb'[\x20-\x7e]{3,}',b)) for c,b in r[3]])
        e+=abs(gs)
