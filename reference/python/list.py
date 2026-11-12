import sys; sys.argv=[sys.argv[0],sys.argv[1]]; exec(open(sys.argv[0].replace('list.py','ccn.py')).read())
cnt=collections.Counter(); szs=collections.Counter()
for c in chunks: cnt[c[0]]+=1; szs[c[0]]+=c[2]
for c in chunks[:0]: pass
print('== chunk summary (id name count totalbytes flags)')
fl={}
for c in chunks: fl.setdefault(c[0],set()).add(c[1])
for k in cnt: print(f'  {k:#06x} {NAMES.get(k,"?"):24} {cnt[k]:5} {szs[k]:12} {sorted(fl[k])}')
