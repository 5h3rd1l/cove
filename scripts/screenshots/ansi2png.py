#!/usr/bin/env python3
"""ANSI (tmux capture-pane -e) -> HTML -> PNG via headless chromium. usage: ansi2png.py cap.txt out.png [page-bg]"""
import re,sys,html,subprocess
src,out=sys.argv[1],sys.argv[2]
bg_page=sys.argv[3] if len(sys.argv)>3 else "#4a171b"
base16=["#000000","#cd3131","#0dbc79","#e5e510","#2472c8","#bc3fbc","#11a8cd","#e5e5e5","#7f7f7f","#f14c4c","#23d18b","#f5f543","#3b8eea","#d670d6","#29b8db","#ffffff"]
def c256(n):
    if n<16: return base16[n]
    if n<232:
        n-=16; r,g,b=n//36,(n//6)%6,n%6; f=lambda v:0 if v==0 else 55+40*v
        return "#%02x%02x%02x"%(f(r),f(g),f(b))
    v=8+10*(n-232); return "#%02x%02x%02x"%(v,v,v)
NEW=lambda: dict(fg=None,bg=None,b=0,d=0,i=0,u=0,r=0)
rows=[]
for line in open(src).read().split("\n"):
    st=NEW(); cells=[]
    for m in re.finditer(r'\x1b\[([0-9;:]*)m|([^\x1b]+)|\x1b[^\[]', line):
        if m.group(2) is not None:
            for ch in m.group(2): cells.append((ch,dict(st)))
        elif m.group(1) is not None:
            p=[int(x) if x else 0 for x in m.group(1).replace(':',';').split(';')]; k=0
            while k<len(p):
                c=p[k]
                if c==0: st=NEW()
                elif c==1: st['b']=1
                elif c==2: st['d']=1
                elif c==3: st['i']=1
                elif c==4: st['u']=1
                elif c==7: st['r']=1
                elif c==22: st['b']=0; st['d']=0
                elif c==23: st['i']=0
                elif c==24: st['u']=0
                elif c==27: st['r']=0
                elif 30<=c<=37: st['fg']=base16[c-30]
                elif 90<=c<=97: st['fg']=base16[c-90+8]
                elif 40<=c<=47: st['bg']=base16[c-40]
                elif 100<=c<=107: st['bg']=base16[c-100+8]
                elif c==39: st['fg']=None
                elif c==49: st['bg']=None
                elif c in (38,48):
                    if p[k+1]==5: col=c256(p[k+2]); k+=2
                    else: col="#%02x%02x%02x"%(p[k+2],p[k+3],p[k+4]); k+=4
                    st['fg' if c==38 else 'bg']=col
                k+=1
    rows.append(cells)
FG="#1c2030" if __import__("os").environ.get("THEME")=="light" else "#d4d4d4"
def span(ch,s):
    fg=s['fg'] or FG; bg=s['bg']
    if s['r']: fg,bg=(bg or bg_page),(s['fg'] or FG)
    css=f"color:{fg};"+(f"background:{bg};" if bg else "")
    if s['b']: css+="font-weight:bold;"
    if s['d']: css+="opacity:.6;"
    if s['i']: css+="font-style:italic;"
    if s['u']: css+="text-decoration:underline;"
    return f'<span style="{css}">{html.escape(ch)}</span>'
body=["".join(span(ch,s) for ch,s in cells) or "&nbsp;" for cells in rows]
import os
FRAME=os.environ.get("FRAME")=="1"
FONT="'JetBrainsMono Nerd Font','JetBrains Mono','DejaVu Sans Mono',monospace"
if FRAME:
    title=os.environ.get("TITLE","cove")
    light=os.environ.get("THEME")=="light"
    chrome="#e9ebf2" if light else "#181b23"; edge="#cfd3e0" if light else "#2b303b"; tcol="#6b7285" if light else "#8b93a7"
    doc=f'''<html><body style="margin:0;background:transparent"><div style="display:inline-block;margin:24px;border-radius:12px;overflow:hidden;border:1px solid {edge};background:{bg_page}">
<div style="height:34px;background:{chrome};display:flex;align-items:center;padding:0 14px;position:relative"><span style="width:12px;height:12px;border-radius:50%;background:#ff5f57;margin-right:8px"></span><span style="width:12px;height:12px;border-radius:50%;background:#febc2e;margin-right:8px"></span><span style="width:12px;height:12px;border-radius:50%;background:#28c840"></span><span style="position:absolute;left:0;right:0;text-align:center;font:13px {FONT};color:{tcol}">{title}</span></div>
<pre style="margin:0;padding:14px 18px 16px;font:15px/19px {FONT};color:{FG};background:{bg_page};display:block">{chr(10).join(body)}</pre></div></body></html>'''
    open(out+".html","w").write(doc)
    subprocess.run(["chromium","--headless","--no-sandbox","--disable-gpu","--hide-scrollbars","--default-background-color=00000000",f"--screenshot={out}","--window-size=1700,1000",out+".html"],capture_output=True)
    subprocess.run(["magick",out,"-trim","+repage","-bordercolor","none","-border","6",out])
else:
    doc=f'''<html><body style="margin:0;background:{bg_page}"><pre style="margin:0;padding:12px;font:15px/19px {FONT};color:{FG};background:{bg_page};display:inline-block">{chr(10).join(body)}</pre></body></html>'''
    open(out+".html","w").write(doc)
    subprocess.run(["chromium","--headless","--no-sandbox","--disable-gpu","--hide-scrollbars",f"--screenshot={out}","--window-size=1420,840",out+".html"],capture_output=True)
print("wrote",out)
