import struct,sys,zlib,collections
p=sys.argv[1]; f=open(p,'rb'); data=f.read()
off=364544
assert data[off:off+4]==b'wwww'
hs,dsz,fmt,_,_,cnt=struct.unpack_from('<IIIIII',data,off+8)
o=off+hs
print('== packed files',cnt)
for i in range(cnt):
    n=struct.unpack_from('<H',data,o)[0]; o+=2
    name=data[o:o+n].decode('latin1'); o+=n
    bingo,sz=struct.unpack_from('<II',data,o); o+=8
    print(f'  {name:30} {sz}'); o+=sz
pame=data.find(b'PAME',o-16)
print('PAME at',pame,'(pack end',o,')')
rv,rsv,pv,pb=struct.unpack_from('<HHII',data,pame+4)
print('runtime',hex(rv),rsv,'product',pv,'build',pb)
o=pame+16
NAMES={0x2223:'AppHeader',0x2224:'AppName',0x2225:'Author',0x2226:'MenuBar',0x2227:'ExtPath',0x2228:'Extensions(old)',0x2229:'FrameItems',0x222A:'GlobalEvents',0x222B:'FrameHandles',0x222C:'ExtData',0x222D:'AdditionalExtension',0x222E:'AppEditorFilename',0x222F:'AppTargetFilename',0x2230:'AppDoc',0x2231:'OtherExts',0x2232:'GlobalValues',0x2233:'GlobalStrings',0x2234:'Extensions',0x2235:'AppIcon',0x2236:'DemoVersion',0x2237:'SecNum',0x2238:'BinaryFiles',0x2239:'AppMenuImages',0x223A:'AboutText',0x223B:'Copyright',0x223C:'GlobalValueNames',0x223D:'GlobalStringNames',0x223E:'MvtTexts',0x223F:'FrameItems2',0x2240:'ExeOnly',0x2242:'Protection',0x2243:'Shaders',0x2245:'ExtendedHeader',0x2246:'Spacer',0x224D:'FrameBank',0x224F:'Chunk224F',0x2251:'TitleText2',0x2253:'ObjectProps2',0x2254:'GlobalEventInfo',0x2255:'?2255',0x2256:'ObjectHeaders2',0x2257:'ObjectNames2',0x2258:'ObjectShaders',
0x3333:'Frame',0x3334:'FrameHeader',0x3335:'FrameName',0x3336:'FramePassword',0x3337:'FramePalette',0x3338:'FrameItemInstances',0x3339:'FrameFadeInFrame',0x333A:'FrameFadeOutFrame',0x333B:'FrameFadeIn',0x333C:'FrameFadeOut',0x333D:'FrameEvents',0x333E:'FramePlayHeader',0x333F:'FrameAdditionalItem',0x3340:'FrameAdditionalItemInstance',0x3341:'FrameLayers',0x3342:'FrameVirtualRect',0x3343:'DemoFilePath',0x3344:'RandomSeed',0x3345:'FrameLayerEffects',0x3346:'BlurayOptions',0x3347:'MvtTimerBase',0x3348:'MosaicImageTable',0x3349:'FrameEffects',0x334A:'FrameIphoneOptions',0x334C:'?334C',
0x4444:'ObjectHeader',0x4445:'ObjectName',0x4446:'ObjectProperties',0x4447:'ObjectChunk4447',0x4448:'ObjectEffect',0x5555:'ImageHandles',0x5556:'FontHandles',0x5557:'SoundHandles',0x5558:'MusicHandles',0x6665:'BankOffsets',0x6666:'ImageBank',0x6667:'FontBank',0x6668:'SoundBank',0x6669:'MusicBank',0x7F7F:'Last'}
chunks=[]
while True:
    cid,fl,sz=struct.unpack_from('<HHI',data,o); o+=8
    chunks.append((cid,fl,sz,o)); o+=sz
    if cid==0x7F7F or o>=len(data): break
print('end of chunks',o,'file',len(data))
def body(c):
    cid,fl,sz,o=c; b=data[o:o+sz]
    if fl&1 and not fl&2:
        dec,cs=struct.unpack_from('<II',b); return zlib.decompress(b[8:8+cs])
    return b if fl==0 else None
