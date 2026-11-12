import sys; exec(open(sys.argv[0].replace('deep.py','ccn.py')).read())
def get(cid): return [c for c in chunks if c[0]==cid]
def s(b): return b.split(b'\0')[0].decode('latin1')
for k in (0x2224,0x2225,0x223b,0x222f,0x222e): print(NAMES[k],':',s(body(get(k)[0])))
h=body(get(0x2223)[0]); print('AppHeader raw',h.hex())
ext=body(get(0x2234)[0]); print('Extensions raw',ext)
print('OtherExts',body(get(0x2231)[0]))
# subchunk parser
def subs(b):
    o=0; out=[]
    while o+8<=len(b):
        if len(out)>100000: raise Exception('runaway')
        cid,fl,sz=struct.unpack_from('<HHI',b,o); o+=8; d=b[o:o+sz]; o+=sz
        if fl&1: d=zlib.decompress(d[8:])
        out.append((cid,d))
        if cid==0x7F7F: break
    return out,o
fi=body(get(0x2229)[0]); n=struct.unpack_from('<I',fi)[0]; o=4
TYPES={-7:'Player',-6:'Keyboard',-5:'Create',-4:'Timer',-3:'Game',-2:'Speaker',-1:'System',0:'QuickBackdrop',1:'Backdrop',2:'Active',3:'String',4:'Q&A',5:'Score',6:'Lives',7:'Counter',8:'RTF',9:'SubApp'}
objs={}; tc=collections.Counter()
for i in range(n):
    sc,used=subs(fi[o:]); o+=used
    hd=dict(sc)
    handle,typ=struct.unpack_from('<Hh',hd[0x4444])
    nm=hd.get(0x4445,b'').split(b'\0')[0].decode('latin1')
    objs[handle]=(nm,typ); tc[TYPES.get(typ,'Ext#%d'%typ)]+=1
print('== object definitions',n); print(dict(tc))
# frames
evtot=collections.Counter(); ac=collections.Counter()
print('== frames')
for fr in get(0x3333):
    sc,_=subs(body(fr)); d=dict(sc)
    name=s(d.get(0x3335,b''))
    w,hgt=struct.unpack_from('<ii',d[0x3334]) if 0x3334 in d else (0,0)
    inst=struct.unpack_from('<I',d[0x3338])[0] if 0x3338 in d else 0
    ev=d.get(0x333D,b''); ng=0; ncond=nact=0
    q=ev.find(b'ERev')
    if q>=0:
        esz=struct.unpack_from('<I',ev,q+4)[0]; e=q+8; end=e+esz
        while e<end:
            gs,nc,na=struct.unpack_from('<hBB',ev,e); ng+=1
            if gs==0: print('   !zero group at',e); break
            p=e+14
            for k in range(nc+na):
                cs,ot,num=struct.unpack_from('<Hhh',ev,p)
                if ot<0 or ot>=32: key=(TYPES.get(ot,'Ext%d'%ot) if ot<32 else 'Ext%d'%ot)
                else: key=TYPES.get(ot)
                (ac if k>=nc else evtot)[(key,num)]+=1
                if cs==0: break
                p+=cs
            ncond+=nc; nact+=na; e+=abs(gs)
    print(f'  {name:28} {w}x{hgt} instances={inst:4} events={ng:4} conds={ncond:5} acts={nact:5}')
print('top object names by type:')
by=collections.defaultdict(list)
for h_,(nm,t) in objs.items(): by[TYPES.get(t,'Ext#%d'%t)].append(nm)
for t,l in by.items(): print(' ',t,len(l),l[:25])
# banks
for k in (0x6666,0x6668,0x6667):
    c=get(k)[0]; print(NAMES[k],'count',struct.unpack_from('<I',data,c[3])[0])

print('== condition usage by object type'); t=collections.Counter()
for (k,n),v in evtot.items(): t[k]+=v
print(t.most_common())
print('== action usage by object type'); t=collections.Counter()
for (k,n),v in ac.items(): t[k]+=v
print(t.most_common())
print('top actions', ac.most_common(25))
print('top conds', evtot.most_common(25))
