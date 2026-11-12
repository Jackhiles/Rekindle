import sys,re; exec(open(sys.argv[0].replace('usage.py','ccn.py')).read())
def subs(b):
    o=0; out=[]
    while o+8<=len(b):
        cid,fl,sz=struct.unpack_from('<HHI',b,o); o+=8; d=b[o:o+sz]; o+=sz
        if fl&1: d=zlib.decompress(d[8:])
        out.append((cid,d))
        if cid==0x7F7F: break
    return out
fi=body([c for c in chunks if c[0]==0x2229][0]); n=struct.unpack_from('<I',fi)[0]; o=4; names={}
for i in range(n):
    sc=[]
    while True:
        cid,fl,sz=struct.unpack_from('<HHI',fi,o); o+=8; d=fi[o:o+sz]; o+=sz
        if fl&1: d=zlib.decompress(d[8:])
        sc.append((cid,d))
        if cid==0x7F7F: break
    hd=dict(sc); h=struct.unpack_from('<H',hd[0x4444])[0]; names[h]=hd.get(0x4445,b'').split(b'\0')[0].decode('latin1')
def toks(b):
    p=2; out=[]
    while p+4<=len(b):
        ot,num=struct.unpack_from('<hh',b,p)
        if ot==0 and num==0: break
        sz=struct.unpack_from('<H',b,p+4)[0]; data=b[p+6:p+sz]
        if ot==-1 and num==0: out.append(('L',struct.unpack_from('<i',data)[0]))
        elif ot==-1 and num==3: out.append(('S',data.split(b'\0')[0].decode('latin1')))
        elif ot==0: out.append(('op',num))
        elif ot>=2: out.append(('obj',ot,num,names.get(struct.unpack_from('<H',data)[0],'?')))
        else: out.append(('sys',ot,num))
        if sz<6: break
        p+=sz
    return out
want=sys.argv[2]
for fr in [c for c in chunks if c[0]==0x3333]:
    d=dict(subs(body(fr))); name=d[0x3335].split(b'\0')[0].decode()
    if name!=want: continue
    ev=d.get(0x333D,b''); q=ev.find(b'ERev'); esz=struct.unpack_from('<I',ev,q+4)[0]; e=q+8; end=e+esz; gi=0
    while e<end:
        gs,nc,na=struct.unpack_from('<hBB',ev,e); p=e+14; gi+=1; rows=[]; hit=False
        for k in range(nc+na):
            cs,ot,num,oi=struct.unpack_from('<HhhH',ev,p)
            hdr=16 if k<nc else 14; npar=ev[p+12]; pp=p+hdr; ps=[]
            for _ in range(npar):
                psz,code=struct.unpack_from('<Hh',ev,pp); b=ev[pp+4:pp+psz]
                t=toks(b) if code in(22,23,27,45) else code
                if isinstance(t,list) and any(x[0]=='obj' and x[1]>=32 for x in t): hit=True
                ps.append(t); pp+=psz
            if ot>=32: hit=True
            rows.append(('IF' if k<nc else '  DO',ot,num,names.get(oi,'') if ot>=0 else '',ps)); p+=cs
        if hit:
            print('--',gi)
            for r in rows: print('  ',*r)
        e+=abs(gs)
