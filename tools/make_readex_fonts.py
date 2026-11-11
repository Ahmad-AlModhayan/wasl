"""Static Readex Pro instances (Regular 400, Medium 500, SemiBold 600, Bold 700) from the variable font,
with a `tnum` feature added: Readex Pro's digits are proportional only, and both Wasl apps ask for
tabular figures wherever a number changes in place (progress, sizes, counts).

Source: https://github.com/google/fonts/tree/main/ofl/readexpro (ReadexPro[HEXP,wght].ttf, OFL 1.1).
Needs fontTools and brotli.

usage: python -I tools/make_readex_fonts.py ReadexPro[HEXP,wght].ttf desktop/public/fonts app/src/main/res/font
"""
import copy
import sys
from pathlib import Path

from fontTools.ttLib import TTFont
from fontTools.ttLib.tables import otTables as ot
from fontTools.varLib import instancer

src, web_dir, android_dir = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
WEIGHTS = {400: 'Regular', 500: 'Medium', 600: 'SemiBold', 700: 'Bold'}
DIGITS = '0123456789' + '٠١٢٣٤٥٦٧٨٩' + '۰۱۲۳۴۵۶۷۸۹'


def add_tabular_digits(font: TTFont) -> int:
    """Adds `<digit>.tf` glyphs, each the same width (the widest digit's) and centred, and a `tnum`
    single substitution to them in every script and language. Returns the tabular width."""
    cmap = font.getBestCmap()
    glyf, hmtx = font['glyf'], font['hmtx']
    names = [cmap[ord(c)] for c in DIGITS if ord(c) in cmap]
    width = max(hmtx[n][0] for n in names)
    order = list(font.getGlyphOrder())
    mapping = {}
    for n in names:
        new = f'{n}.tf'
        adv, lsb = hmtx[n]
        dx = (width - adv) // 2
        g = copy.deepcopy(glyf[n])
        if g.isComposite():
            for c in g.components:
                c.x += dx
        elif g.numberOfContours > 0:
            g.coordinates.translate((dx, 0))
        glyf.glyphs[new] = g
        order.append(new)
        g.recalcBounds(glyf)
        hmtx[new] = (width, (g.xMin if hasattr(g, 'xMin') else lsb + dx))
        mapping[n] = new
    font.setGlyphOrder(order)
    glyf.glyphOrder = order
    font['maxp'].numGlyphs = len(order)
    if 'post' in font and hasattr(font['post'], 'extraNames'):
        font['post'].formatType = 2.0

    gsub = font['GSUB'].table
    sub = ot.SingleSubst()
    sub.mapping = mapping
    lookup = ot.Lookup()
    lookup.LookupType, lookup.LookupFlag, lookup.SubTable = 1, 0, [sub]
    lookup.SubTableCount = 1
    gsub.LookupList.Lookup.append(lookup)
    gsub.LookupList.LookupCount = len(gsub.LookupList.Lookup)
    lookup_index = gsub.LookupList.LookupCount - 1

    # Insert the feature record in tag order and shift every index that pointed past it.
    records = gsub.FeatureList.FeatureRecord
    at = next((i for i, r in enumerate(records) if r.FeatureTag > 'tnum'), len(records))
    rec = ot.FeatureRecord()
    rec.FeatureTag = 'tnum'
    rec.Feature = ot.Feature()
    rec.Feature.FeatureParams = None
    rec.Feature.LookupListIndex = [lookup_index]
    rec.Feature.LookupCount = 1
    records.insert(at, rec)
    gsub.FeatureList.FeatureCount = len(records)
    for script in gsub.ScriptList.ScriptRecord:
        langs = [script.Script.DefaultLangSys] + [l.LangSys for l in script.Script.LangSysRecord]
        for ls in langs:
            if ls is None:
                continue
            ls.FeatureIndex = sorted([i + 1 if i >= at else i for i in ls.FeatureIndex] + [at])
            ls.FeatureCount = len(ls.FeatureIndex)
    if getattr(gsub, 'FeatureVariations', None):
        for r in gsub.FeatureVariations.FeatureVariationRecord:
            for s in r.FeatureTableSubstitution.SubstitutionRecord:
                if s.FeatureIndex >= at:
                    s.FeatureIndex += 1
    return width


var = TTFont(src)
axes = {a.axisTag: (a.minValue, a.defaultValue, a.maxValue) for a in var['fvar'].axes}
cmap = var.getBestCmap()
for ch in 'Aaز٠١٢،؛؟ـ':
    assert ord(ch) in cmap, f'missing {ch!r}'

for weight, style in WEIGHTS.items():
    font = TTFont(src)
    pins = {'wght': weight}
    if 'HEXP' in axes:
        pins['HEXP'] = axes['HEXP'][1]
    inst = instancer.instantiateVariableFont(font, pins, updateFontNames=True)
    width = add_tabular_digits(inst)
    name = inst['name']
    for rec in list(name.names):
        if rec.nameID in (1, 2, 4, 6, 16, 17):
            name.removeNames(nameID=rec.nameID)
    name.setName('Readex Pro', 1, 3, 1, 0x409)
    name.setName('Regular', 2, 3, 1, 0x409)
    name.setName(f'Readex Pro {style}', 4, 3, 1, 0x409)
    name.setName(f'ReadexPro-{style}', 6, 3, 1, 0x409)
    name.setName('Readex Pro', 16, 3, 1, 0x409)
    name.setName(style, 17, 3, 1, 0x409)
    inst['OS/2'].usWeightClass = weight
    ttf = android_dir / f'readex_pro_{style.lower()}.ttf'
    inst.save(ttf)
    inst.flavor = 'woff2'
    inst.save(web_dir / f'ReadexPro-{style}.woff2')

    # Check what was written: tnum present, digits tabular, nothing else lost.
    back = TTFont(ttf)
    tags = [r.FeatureTag for r in back['GSUB'].table.FeatureList.FeatureRecord]
    assert 'tnum' in tags and tags == sorted(tags), tags
    sub = back['GSUB'].table.LookupList.Lookup[-1].SubTable[0].mapping
    widths = {back['hmtx'][g][0] for g in sub.values()}
    assert widths == {width}, widths
    print(style, 'tabular width', width, 'digits', len(sub), 'features', len(tags),
          (web_dir / f'ReadexPro-{style}.woff2').stat().st_size, ttf.stat().st_size)
