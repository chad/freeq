import sys, wave, struct, math, array
path = sys.argv[1]
fps = float(sys.argv[2]) if len(sys.argv) > 2 else 15.0
w = wave.open(path, 'rb')
sr, ch, sw, n = w.getframerate(), w.getnchannels(), w.getsampwidth(), w.getnframes()
raw = w.readframes(n)
a = array.array('h'); a.frombytes(raw)
if ch > 1: a = a[0::ch]
spf = max(1, int(sr / fps))
levels = []
for i in range(0, len(a), spf):
    chunk = a[i:i+spf]
    if not chunk: break
    s = sum(v*v for v in chunk)
    levels.append(math.sqrt(s/len(chunk)) / 32768.0)
mx = max(levels + [1e-4])
# light smoothing so the mouth doesn't chatter on every sample
out = []
prev = 0.0
for l in levels:
    v = min(1.0, (l/mx)**0.7 * 1.15)
    v = 0.55*v + 0.45*prev  # attack/decay smoothing
    prev = v
    out.append(v)
print("\n".join(f"{v:.4f}" for v in out))
