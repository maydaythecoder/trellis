"""How many Pyright-bound edges name a method a first-party subclass overrides.

Input: the output of `cargo run --release -p trellis-index --example
backstop_edges -- <root>`. For each bound edge whose callee is a method, it
walks the subclasses of the callee's class (transitively) and asks whether any
of them defines a method of the same name. When one does, the edge names the
declared type's member, and ADR 0012 treats a declared type as an upper bound:
the resolver would have kept the overrides as `Candidates`.

A text proxy, not the resolver's hierarchy: classes are matched by bare name
and bases are read from the `class` line, so a name shared by two classes
merges them.

It also counts callees that cannot be the runtime callee at all: an
`@abstractmethod`, or a body of `...`, `pass` or `raise NotImplementedError`
(after any docstring). Usage: python backstop_overrides.py <root> <edges.txt>
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
cache = {}
def src(p):
    if p not in cache: cache[p] = (root/p).read_text(encoding='utf-8').splitlines()
    return cache[p]
def abstract(p, ln):
    L = src(p); i = ln-1
    # decorators above
    j = i-1; decos = []
    while j >= 0 and L[j].strip().startswith('@'): decos.append(L[j].strip()); j -= 1
    if any('abstractmethod' in d for d in decos): return 'abstractmethod'
    # the def header may span lines; find the line ending with ':' at depth 0 (approx: first line whose rstrip ends with ':' or contains ': ...')
    k = i
    while k < len(L) and not re.search(r':\s*(\.\.\.)?\s*(#.*)?$', L[k]): k += 1
    if k < len(L) and re.search(r':\s*\.\.\.\s*$', L[k]): return 'stub ...'
    body = [x.strip() for x in L[k+1:k+8] if x.strip() and not x.strip().startswith('#')]
    while body and (body[0].startswith('"""') or body[0].startswith("'''")):
        # skip docstring
        q = body[0][:3]; 
        if body[0].count(q) >= 2 and len(body[0])>3: body = body[1:]
        else:
            body = body[1:]
            while body and q not in body[0]: body = body[1:]
            body = body[1:]
    if body and (body[0] in ('...','pass') or body[0].startswith('raise NotImplementedError')): return 'stub body: ' + body[0][:30]
    return None
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
    if abstract(e[4], e[5]): tot[kind+' -> abstract or stub body']+=1
    cls=where.get((e[4],e[5]))
    if cls is None: continue
    tot[kind+' method']+=1
    s=overridden(cls,e[3])
    if s: tot[kind+' -> overridden in a subclass']+=1; ex[kind].append((e[0],e[1],e[2],cls+'.'+e[3],'overridden in '+s))
print(tot)
for k,v in ex.items(): print(k, v[:4])
