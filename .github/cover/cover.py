"""Writes .github/cover.svg, fonts embedded: python3 .github/cover/cover.py"""
import base64
import math
import pathlib
import random
import re
import urllib.request

LANGUAGES = [
    ("Rust", "#e8a47c"),
    ("TypeScript", "#4f93e6"),
    ("Python", "#f4d35e"),
    ("Go", "#3fc6e8"),
    ("Java", "#f5873a"),
]
TAGLINE = "OpenAPI in, idiomatic SDKs out."
SUBLINE = "GitHub-native SDK generation. No cloud, no subscription."

W, H = 1280, 640
RX, RY = 1000, 150
random.seed(7)
stars = "".join(
    f'<circle cx="{random.uniform(0,W):.1f}" cy="{random.uniform(0,H):.1f}" r="{random.uniform(.4,1.5):.2f}" fill="#fff" opacity="{random.uniform(.15,.8):.2f}"/>'
    for _ in range(170))
meteors, chips = [], []
for i, (name, color) in enumerate(LANGUAGES):
    t = i / max(len(LANGUAGES) - 1, 1)
    deg, dist = 146 - 90 * t, 270 + 60 * math.sin(math.pi * t)
    a = math.radians(deg)
    dx, dy = math.cos(a), math.sin(a)
    x0, y0 = RX + dx * 70, RY + dy * 70
    x1, y1 = RX + dx * dist, RY + dy * dist
    gid = f"g{i}"
    meteors.append(f'''<linearGradient id="{gid}" x1="{x0:.1f}" y1="{y0:.1f}" x2="{x1:.1f}" y2="{y1:.1f}" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="{color}" stop-opacity="0"/><stop offset=".75" stop-color="{color}" stop-opacity=".55"/><stop offset="1" stop-color="#fff" stop-opacity="1"/></linearGradient>
    <line x1="{x0:.1f}" y1="{y0:.1f}" x2="{x1:.1f}" y2="{y1:.1f}" stroke="url(#{gid})" stroke-width="3" stroke-linecap="round"/>
    <circle cx="{x1:.1f}" cy="{y1:.1f}" r="9" fill="{color}" opacity=".35" filter="url(#blur)"/>
    <circle cx="{x1:.1f}" cy="{y1:.1f}" r="3.2" fill="#fff"/>''')
    cw = 44 + len(name) * 9.3
    cx, cy = x1 + dx * 22, y1 + dy * 22
    left = cx - cw / 2 if abs(dx) < .5 else (cx if dx > 0 else cx - cw)
    top = cy - 17 if abs(dy) < .6 or dy < 0 else cy - 6
    chips.append(f'''<g transform="translate({left:.1f},{top:.1f})">
      <rect width="{cw:.1f}" height="34" rx="17" fill="#0a0e1c" stroke="{color}" stroke-opacity=".6"/>
      <circle cx="17" cy="17" r="5" fill="{color}"/>
      <text x="30" y="22.5" class="chip">{name}</text></g>''')
faint = []
for deg in (10, 32, 200, 222, 250, 274, 298, 322, 342):
    a = math.radians(deg); L = random.uniform(120, 260); s = random.uniform(60, 140)
    x0, y0 = RX + math.cos(a) * s, RY + math.sin(a) * s
    x1, y1 = RX + math.cos(a) * (s + L), RY + math.sin(a) * (s + L)
    faint.append(f'<line x1="{x0:.1f}" y1="{y0:.1f}" x2="{x1:.1f}" y2="{y1:.1f}" stroke="url(#faint{deg})" stroke-width="1.4" stroke-linecap="round"/>'
                 f'<linearGradient id="faint{deg}" x1="{x0:.1f}" y1="{y0:.1f}" x2="{x1:.1f}" y2="{y1:.1f}" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#c9d3f0" stop-opacity="0"/><stop offset="1" stop-color="#e3e9ff" stop-opacity=".4"/></linearGradient>')
svg = f'''<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
  <defs>
    <radialGradient id="sky" cx="{RX/W:.3f}" cy="{RY/H:.3f}" r="1.05">
      <stop offset="0" stop-color="#18203a"/><stop offset=".4" stop-color="#0b0f1d"/><stop offset="1" stop-color="#030409"/></radialGradient>
    <linearGradient id="word" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#c3cdf0"/></linearGradient>
    <filter id="blur" x="-2" y="-2" width="5" height="5"><feGaussianBlur stdDeviation="6"/></filter>
    <filter id="glow" x="-1" y="-1" width="3" height="3"><feGaussianBlur stdDeviation="28"/></filter>
  </defs>
  <rect width="{W}" height="{H}" fill="url(#sky)"/>
  {stars}
  {"".join(faint)}
  {"".join(meteors)}
  <circle cx="{RX}" cy="{RY}" r="58" fill="#7d8fd8" opacity=".32" filter="url(#glow)"/>
  <circle cx="{RX}" cy="{RY}" r="46" fill="#080b16" stroke="#9fb0e8" stroke-opacity=".55" stroke-width="1.5"/>
  <text x="{RX}" y="{RY+11}" text-anchor="middle" class="brace">{{ }}</text>
  <text x="{RX}" y="{RY-62}" text-anchor="middle" class="label">openapi.json</text>
  {"".join(chips)}
  <text x="84" y="286" class="word">perseid</text>
  <text x="88" y="348" class="tag">{TAGLINE}</text>
  <text x="88" y="392" class="sub">{SUBLINE}</text>
  <g transform="translate(86,440)">
    <rect width="484" height="46" rx="10" fill="#080b16" stroke="#232a44"/>
    <text x="20" y="29" class="cmd"><tspan fill="#8fa3e0">$</tspan> curl -fsSL sh.meteroid.com/perseid | sh</text>
  </g>
  <text x="88" y="590" class="foot">github.com/meteroid-oss/perseid</text>
</svg>'''

def get(url):
    agent = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Safari/537.36"
    return urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": agent})).read()


css = get("https://fonts.googleapis.com/css2?family=Inter:wght@400;600;800&family=JetBrains+Mono:wght@500;700").decode()
fonts = ""
for body in re.findall(r"/\* latin \*/\s*@font-face \{(.*?)\}", css, re.S):
    url = re.search(r"url\((.*?)\)", body).group(1)
    fonts += "@font-face {" + body.replace(url, "data:font/woff2;base64," + base64.b64encode(get(url)).decode()) + "}"
style = f"""<style>{fonts}
.word{{font:800 120px Inter;fill:url(#word);letter-spacing:-5px}}
.tag{{font:600 34px Inter;fill:#eef1fb;letter-spacing:-.5px}}
.sub{{font:400 20px Inter;fill:#8e97b8}}
.cmd{{font:500 18px 'JetBrains Mono';fill:#dde3f5}}
.chip{{font:600 17px Inter;fill:#eef0ff}}
.brace{{font:700 34px 'JetBrains Mono';fill:#e6ebff}}
.label{{font:500 14px 'JetBrains Mono';fill:#8f9cc9;letter-spacing:.5px}}
.foot{{font:500 16px 'JetBrains Mono';fill:#4f5775}}
</style>"""
out = pathlib.Path(__file__).parent.parent / "cover.svg"
out.write_text(svg.replace("<defs>", style + "<defs>", 1))
print(f"wrote {out}")
