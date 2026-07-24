with open('adk-realtime/examples/bench_audio_buffering.rs', 'r') as f:
    content = f.read()

content = content.replace('if _ == 0 { ALLOC_COUNT.store(0, Ordering::SeqCst); }', 'ALLOC_COUNT.store(0, Ordering::SeqCst);', 2)
content = content.replace('if _ == 0 { ALLOC_BYTES.store(0, Ordering::SeqCst); }', 'ALLOC_BYTES.store(0, Ordering::SeqCst);', 2)

lines = content.split('\n')
new_lines = []
for line in lines:
    if 'if _ == 0 { ALLOC_COUNT.store(0, Ordering::SeqCst); }' in line:
        pass # remove
    elif 'if _ == 0 { ALLOC_BYTES.store(0, Ordering::SeqCst); }' in line:
        pass # remove
    else:
        new_lines.append(line)

with open('adk-realtime/examples/bench_audio_buffering.rs', 'w') as f:
    f.write('\n'.join(new_lines))
