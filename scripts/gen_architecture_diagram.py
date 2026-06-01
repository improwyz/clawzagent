#!/usr/bin/env python3
"""Generate the ClawZ platform architecture diagram (dark theme, silver logo).

Renders at 2x supersampling for crisp anti-aliased text, then downscales.
Output: docs/clawz-architecture-v1.1.png
"""
from PIL import Image, ImageDraw, ImageFont

S = 2  # supersample factor
W = 1740

# ── palette (dark theme; silver + copper brand accents) ─────────────────────
BG       = "#0E1117"
PANEL    = "#161B22"
PANEL_BD = "#2B313B"
CHIP     = "#1C2230"
CHIP_BD  = "#39414F"
TEXT     = "#E6E8EB"
MUTED    = "#9BA4B0"
SILVER   = "#CBD2DA"
COPPER   = "#C77B3C"
ARROW    = "#5B6675"

# layer accents
A_CLIENT = "#2F81F7"  # blue
A_EDGE   = "#A371F7"  # purple (security edge)
A_EXEC   = "#3FB950"  # green (execution)
A_CASC   = "#39C5CF"  # teal (cascade)
A_SHARE  = "#8B949E"  # slate (shared libs)
A_DATA   = "#D29922"  # amber (persistence)
A_EXT    = "#C77B3C"  # copper (external egress)

FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
FONTB = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
FONTM = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"

def f(path, size):
    return ImageFont.truetype(path, size * S)

font_title = f(FONTB, 30)
font_sub   = f(FONT, 15)
font_band  = f(FONTB, 17)
font_bandt = f(FONT, 12)        # band tagline
font_chip  = f(FONTB, 13)
font_chips = f(FONT, 11)        # small chip / sub-label
font_tb    = f(FONTB, 12)       # trust boundary
font_foot  = f(FONT, 12)
font_arrow = f(FONT, 11)

def sx(v):  # scale a logical coord
    return int(v * S)

# ── bands: (title, tagline, accent, rows) ; row = list of (label, sublabel|None)
bands = [
    ("CLIENTS  &  CHANNELS", "user + machine surfaces", A_CLIENT, [
        [("Web Dashboard","React / Vite"),("CLI","clawz"),("TUI","ratatui"),
         ("Tauri Desktop",None),("Embedded","no_std")],
        [("Slack",None),("Discord",None),("Teams",None),("Twilio / Voice",None),
         ("WhatsApp",None),("Inbound Webhooks","HMAC-signed")],
    ]),
    ("EDGE  —  clawz-gateway  (Axum)", "Trust Boundary #1  ·  authn/z + abuse control", A_EDGE, [
        [("CORS allowlist",None),("Security headers","HSTS / CSP"),
         ("Rate limit","per-node + Postgres"),("Idempotency","Idempotency-Key"),
         ("Body-size cap",None),("Auth","JWT + API key"),("SSRF guard",None),("Audit log",None)],
        [("/api/v1/* ","agents·rooms·tools·providers·governance·fleet·cloud·system·setup"),
         ("/ws","run events"),("/webhooks","channels"),("/mcp","tools")],
    ]),
    ("EXECUTION  —  clawz-worker", "Trust Boundary #2  ·  prompt-injection + tool surface", A_EXEC, [
        [("Receive",None),("Context",None),("Governance",None),("Provider",None),
         ("Tools",None),("Persist",None),("Stream",None)],
        [("Provider Router","CircuitBreaker · retry · cost"),
         ("Governance","guardrails · approval · hash-chain audit"),
         ("Channels","SDK + adapters"),("Mesh","P2P · fleet · discovery"),
         ("Orchestration","Docker / bollard")],
    ]),
    ("3-TIER  ELASTIC  CASCADE", "orchestrator spawns agents; agents spawn tool/MCP pods", A_CASC, [
        [("Orchestrator",None),("⇒  Agent containers","tenant-isolated"),
         ("⇒  Tool / MCP pods","elastic, ephemeral")],
    ]),
    ("SHARED  CRATES", "common contracts reused across services", A_SHARE, [
        [("clawz-core","types · db · config · circuit_breaker · retry · prism"),
         ("clawz-services","DTOs · execution client · store"),
         ("clawz-platform","capability tiers t0–t3")],
    ]),
    ("PERSISTENCE", "single Postgres — no extra infra", A_DATA, [
        [("Postgres + TimescaleDB + pgvector",
          "agents · conversations · messages · rooms · users · audit · rate_limit_counters · idempotency_keys"),
         ("In-memory AppState","registries / hot caches")],
    ]),
    ("EXTERNAL  EGRESS", "Trust Boundary #3  ·  guarded outbound", A_EXT, [
        [("LLM providers","OpenAI · Anthropic · Gemini · Bedrock · Ollama · DeepSeek · Azure")],
        [("35 SaaS connectors","OAuth2 token endpoints"),
         ("15 cloud deployers",None),("Channel webhooks","outbound")],
    ]),
]

# arrow labels between consecutive bands
arrows = ["HTTPS / WSS", "gRPC / in-process", "Docker API", "uses", "SQL (sqlx)", "HTTPS  (SSRF-checked)"]

# ── measure layout heights ──────────────────────────────────────────────────
HEADER_H = 150
GAP = 46
PAD = 30          # canvas margin
TITLE_H = 34      # band title strip
ROW_H = 50        # chip row height
ROW_GAP = 12

def band_height(rows):
    return TITLE_H + 14 + len(rows) * ROW_H + (len(rows) - 1) * ROW_GAP + 16

heights = [band_height(r) for (_, _, _, r) in bands]
total_h = PAD + HEADER_H + GAP + sum(heights) + GAP * (len(bands) - 1) + PAD
H = total_h

# ── canvas (supersampled) ─────────────────────────────────────────────────
img = Image.new("RGB", (sx(W), sx(H)), BG)
d = ImageDraw.Draw(img)

def rrect(x0, y0, x1, y1, r, fill=None, outline=None, width=1):
    d.rounded_rectangle([sx(x0), sx(y0), sx(x1), sx(y1)], radius=sx(r),
                        fill=fill, outline=outline, width=max(1, sx(width)))

def text(x, y, s, font, fill, anchor="la"):
    d.text((sx(x), sx(y)), s, font=font, fill=fill, anchor=anchor)

def tw(s, font):
    return d.textlength(s, font=font) / S

# ── header: logo + title ──────────────────────────────────────────────────
logo = Image.open("docs/Logo/clawz-silver-f.png").convert("RGBA")
logo_h = 104
logo_w = int(logo_h * logo.width / logo.height)
logo_r = logo.resize((sx(logo_w), sx(logo_h)), Image.LANCZOS)
ly = PAD + (HEADER_H - logo_h) // 2 - 6
img.paste(logo_r, (sx(PAD + 6), sx(ly)), logo_r)

tx = PAD + logo_w + 40
text(tx, PAD + 40, "Platform Architecture", font_title, TEXT)
text(tx, PAD + 84, "Agent orchestration platform  ·  Rust workspace (11 crates)  ·  v1.1.0",
     font_sub, MUTED)
# copper rule under header
d.rectangle([sx(PAD), sx(PAD + HEADER_H), sx(W - PAD), sx(PAD + HEADER_H) + max(1, sx(2))],
            fill=COPPER)

# ── draw bands ──────────────────────────────────────────────────────────────
y = PAD + HEADER_H + GAP
band_centers = []
for i, (title, tagline, accent, rows) in enumerate(bands):
    h = heights[i]
    x0, x1 = PAD, W - PAD
    # panel
    rrect(x0, y, x1, y + h, 12, fill=PANEL, outline=PANEL_BD, width=2)
    # accent bar (left)
    rrect(x0, y, x0 + 8, y + h, 4, fill=accent)
    # title strip
    text(x0 + 26, y + 9, title, font_band, TEXT)
    tagw = tw(tagline, font_bandt)
    # trust-boundary taglines in copper, others muted
    tagcol = COPPER if "Trust Boundary" in tagline else MUTED
    text(x1 - 16, y + 12, tagline, font_bandt, tagcol, anchor="ra")
    d.line([sx(x0 + 22), sx(y + TITLE_H), sx(x1 - 14), sx(y + TITLE_H)],
           fill=PANEL_BD, width=max(1, sx(1)))

    # chip rows
    ry = y + TITLE_H + 12
    for row in rows:
        # compute widths
        gap = 14
        widths = []
        for (label, sub) in row:
            lw = tw(label, font_chip)
            sw = tw(sub, font_chips) if sub else 0
            widths.append(max(lw, sw) + 30)
        total = sum(widths) + gap * (len(row) - 1)
        cx = x0 + 26
        # if row too wide, shrink padding implicitly by left-align (it fits by design)
        for (label, sub), wdt in zip(row, widths):
            ch = ROW_H - 8
            rrect(cx, ry, cx + wdt, ry + ch, 8, fill=CHIP, outline=CHIP_BD, width=1)
            # accent dot
            d.ellipse([sx(cx + 11) - sx(3), sx(ry + ch/2) - sx(3),
                       sx(cx + 11) + sx(3), sx(ry + ch/2) + sx(3)], fill=accent)
            if sub:
                text(cx + 22, ry + 8, label, font_chip, TEXT)
                text(cx + 22, ry + 26, sub, font_chips, MUTED)
            else:
                text(cx + 22, ry + ch/2, label, font_chip, TEXT, anchor="lm")
            cx += wdt + gap
        ry += ROW_H + ROW_GAP

    band_centers.append((y, y + h))
    # arrow to next band
    if i < len(bands) - 1:
        ax = W / 2
        ay0 = y + h + 6
        ay1 = y + h + GAP - 6
        d.line([sx(ax), sx(ay0), sx(ax), sx(ay1)], fill=ARROW, width=max(1, sx(2)))
        # arrowhead
        d.polygon([(sx(ax), sx(ay1 + 4)), (sx(ax - 6), sx(ay1 - 6)),
                   (sx(ax + 6), sx(ay1 - 6))], fill=ARROW)
        # label
        lbl = arrows[i]
        lw = tw(lbl, font_arrow)
        d.rectangle([sx(ax + 14), sx((ay0+ay1)/2 - 9), sx(ax + 14 + lw + 12), sx((ay0+ay1)/2 + 9)],
                    fill=BG)
        text(ax + 20, (ay0 + ay1) / 2, lbl, font_arrow, MUTED, anchor="lm")
    y += h + GAP

# ── footer ────────────────────────────────────────────────────────────────
text(PAD + 2, H - PAD + 2,
     "Trust boundaries:  #1 gateway edge  ·  #2 worker execution  ·  #3 external egress."
     "   Generated for ClawZ v1.1.0.",
     font_foot, MUTED, anchor="lb")
text(W - PAD, H - PAD + 2, "© Enterpryz Ventures — ClawZ", font_foot, COPPER, anchor="rb")

# ── downscale + save ─────────────────────────────────────────────────────
out = img.resize((W, H), Image.LANCZOS)
out.save("docs/clawz-architecture-v1.1.png")
print(f"wrote docs/clawz-architecture-v1.1.png  ({W}x{H})")
