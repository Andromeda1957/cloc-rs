#!/usr/bin/env python3
"""Compare actual CLI wall time with identical arguments and warmup policy."""
import argparse
import statistics
import subprocess
import time

p=argparse.ArgumentParser()
p.add_argument('--binary',default='cloc-rs')
p.add_argument('--reference',default='cloc')
p.add_argument('--runs',type=int,default=5)
p.add_argument('inputs',nargs='+')
args=p.parse_args()
if args.runs<1:p.error('--runs must be positive')

results={}
for tool in [args.binary,args.reference]:
    command=[tool,'--quiet','--hide-rate',*args.inputs]
    subprocess.run(command,stdout=subprocess.DEVNULL,check=True)
    samples=[]
    for _ in range(args.runs):
        start=time.perf_counter()
        subprocess.run(command,stdout=subprocess.DEVNULL,check=True)
        samples.append(time.perf_counter()-start)
    results[tool]=statistics.median(samples)
    print(f'{tool}: median {results[tool]:.4f} s ({args.runs} runs)')
print(f'cloc-rs speedup: {results[args.reference]/results[args.binary]:.2f}x')
