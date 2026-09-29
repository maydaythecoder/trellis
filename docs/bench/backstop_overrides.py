"""How many Pyright-bound edges name a method a first-party subclass overrides.

Input: the output of `cargo run --release -p trellis-index --example
backstop_edges -- <root>`. For each bound edge whose callee is a method, it
walks the subclasses of the callee's class (transitively) and asks whether any
of them defines a method of the same name. When one does, the edge names the
declared type's member, and ADR 0012 treats a declared type as an upper bound:
the resolver would have kept the overrides as `Candidates`.

A text proxy, not the resolver's hierarchy: classes are matched by bare name
and bases are read from the `class` line, so a name shared by two classes
merges them. Usage: python backstop_overrides.py <root> <edges.txt>
"""
import re, sys, pathlib, collections
root = pathlib.Path(sys.argv[1]); lines = open(sys.argv[2], encoding='utf-8').read().splitlines()
edges = []
for a, b in zip(lines, lines[1:]):
    m = re.match(r'^(\S+):(\d+)  .*\[(Unresolved|Candidates\(\d+\))\]\s*$', a)
    n = re.match(r'^    -> (\S+) at (\S+):(\d+)', b)
    if b.startswith('    -> ') and not m: print('UNPARSED', a)
    if m and n: edges.append((m.group(1), int(m.group(2)), m.group(3), n.group(1), n.group(2), int(n.group(3))))
classes = {}  # name -> list of (file, bases, methods)
subs = collections.defaultdict(set)
where = {}
for f in root.rglob('*.py'):
    rel = f.relative_to(root).as_posix()
    L = f.read_text(encoding='utf-8', errors='replace').splitlines()
    stack = []
    for i, l in enumerate(L):
        m = re.match(r'^(\s*)class (\w+)\s*(\((.*?)\))?\s*:', l)
        ind = len(l) - len(l.lstrip())
        if l.strip():
            while stack and stack[-1][0] >= ind and not l.lstrip().startswith(('#',')')): stack.pop()
        if m:
            name = m.group(2); bases = [b.strip().split('[')[0].split('.')[-1] for b in (m.group(4) or '').split(',') if b.strip() and '=' not in b]
            for b in bases: subs[b].add(name)
            classes.setdefault(name, set())
            stack.append((ind, name)); continue
        d = re.match(r'^(\s*)(async\s+)?def (\w+)', l)
        if d and stack and stack[-1][0] < len(d.group(1)):
            classes[stack[-1][1]].add(d.group(3)); where[(rel, i+1)] = stack[-1][1]
def overridden(cls, meth):
    seen=set(); todo=[cls]
    while todo:
        c=todo.pop()
        for s in subs.get(c,()):
            if s in seen: continue
            seen.add(s)
            if meth in classes.get(s,()): return s
            todo.append(s)
    return None
tot=collections.Counter(); ex=collections.defaultdict(list)
for e in edges:
    kind='Candidates' if e[2].startswith('Cand') else 'Unresolved'
    tot[kind]+=1
    cls=where.get((e[4],e[5]))
    if cls is None: continue
    tot[kind+' method']+=1
    s=overridden(cls,e[3])
    if s: tot[kind+' -> overridden in a subclass']+=1; ex[kind].append((e[0],e[1],e[2],cls+'.'+e[3],'overridden in '+s))
print(tot)
for k,v in ex.items(): print(k, v[:4])
