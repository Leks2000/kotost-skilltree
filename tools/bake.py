#!/usr/bin/env python3
"""Bakes Passive Skill Tree's own data files (skills, lang, textures) into kotost_skilltree/data.

    python -I bake.py <PassiveSkillTree jar> <out data dir>

Reads only the jar's data and assets (JSON, PNG), never its code. Writes:
  atlas.png  1024x1024: icons (16px grid), the five button frames, the background.
  tree.json  the skills: position, frame, icon, links, ru/en tooltip lines, simplified effects.
"""
import io, json, re, sys, zipfile
from PIL import Image

jar, out = sys.argv[1], sys.argv[2]
z = zipfile.ZipFile(jar)
def jload(p): return json.loads(z.read(p).decode("utf-8"))
LANG = {"ru": jload("assets/skilltree/lang/ru_ru.json"), "en": jload("assets/skilltree/lang/en_us.json")}

VAN_ATTR = {
    "armor": ("Броня", "Armor"), "armor_toughness": ("Твёрдость брони", "Armor Toughness"),
    "attack_damage": ("Урон", "Attack Damage"), "attack_speed": ("Скорость атаки", "Attack Speed"),
    "luck": ("Удача", "Luck"), "max_health": ("Максимальное здоровье", "Max Health"),
    "movement_speed": ("Скорость", "Speed"),
}
EFFECTS = {"minecraft:poison": ("Отравление", "Poison")}
EXTRA = {"curios:ring": ("Слоты колец", "Ring Slots")}
missing = set()

def fmt(v):
    s = ("%.2f" % v).rstrip("0").rstrip(".")
    return s if s else "0"

def L(lang, key, *args, default=None):
    t = LANG[lang].get(key)
    if t is None:
        t = LANG["en"].get(key)
    if t is None:
        if default is not None:
            return default
        missing.add(key)
        t = key + " %s" * len(args)
    it = iter(args)
    def sub(m):
        if m.group(0) == "%%":
            return "%"
        return str(next(it, ""))
    return re.sub(r"%%|%[sd]", sub, t)

def attr_name(lang, a):
    i = 0 if lang == "ru" else 1
    if a in EXTRA:
        return EXTRA[a][i]
    if a.startswith("skilltree:"):
        return L(lang, "attribute.name.skilltree." + a.split(":")[1])
    short = a.split(":")[1].replace("generic.", "")
    if short in VAN_ATTR:
        return VAN_ATTR[short][i]
    missing.add(a)
    return short

def effect_name(lang, e):
    if e in EFFECTS:
        return EFFECTS[e][0 if lang == "ru" else 1]
    return L(lang, "effect." + e.replace(":", "."))

def item_key(c):
    t = c["type"].split(":")[1]
    if t == "equipment_type": return "item_condition.skilltree.equipment_type." + c["equipment_type"]
    if t == "potion": return "item_condition.skilltree.potion." + c["potion_type"]
    if t == "food": return "item_condition.skilltree.food"
    if t == "tag": return "item_tag." + c["tag_id"]
    return "item_condition.skilltree.none"

def item_name(lang, c, variant=""):
    t = c["type"].split(":")[1]
    if t == "enchanted":
        inner = c["item_condition"]
        v = "plural.type" if variant.startswith("plural") else "type"
        return L(lang, "item_condition.skilltree.enchanted", item_name(lang, inner, v) if lang == "ru" else item_name(lang, inner, variant))
    key = item_key(c)
    chain = [variant] if variant else []
    if variant == "plural.type": chain += ["plural"]
    for v in chain:
        s = LANG[lang].get(key + "." + v) or LANG["en"].get(key + "." + v)
        if s is not None:
            return s
    base = L(lang, key)
    if lang == "ru" and variant in ("type", "plural.type"):
        return " " + base  # the template glues a gendered ending; without one keep a space
    return base

def none(c): return c is None or c.get("type") == "skilltree:none"

def num(amount, op, name):
    sign = "+" if amount >= 0 else "-"
    a = abs(amount)
    if op == 0:
        return "%s%s %s" % (sign, fmt(a), name)
    return "%s%s%% %s" % (sign, fmt(a * 100), name)

def living(lang, text, c, who):
    if none(c): return text
    t = c["type"].split(":")[1]
    k = "living_condition.skilltree." + t
    tgt = L(lang, k + ".target." + who)
    if t in ("has_item_in_hand", "has_item_equipped"):
        return L(lang, k, text, tgt, item_name(lang, c["item_condition"]))
    if t == "has_effect":
        return L(lang, k + (".amplifier" if c.get("amplifier", 0) > 0 else ""), text, tgt, effect_name(lang, c["effect"]))
    if t in ("health_percentage", "food_level", "attribute_value", "effect_amount", "has_gems"):
        lo, hi = c.get("min", -1), c.get("max", -1)
        pct = (lambda v: fmt(v * 100)) if t == "health_percentage" else fmt
        tail = []
        if t == "attribute_value": tail = [attr_name(lang, c["attribute"])]
        if t == "has_gems": tail = [item_name(lang, c["item_condition"], "where")]
        if lo != -1 and hi != -1: return L(lang, k + ".range", text, tgt, pct(lo), pct(hi), *tail)
        if lo != -1:
            if lo == 1 and t in ("effect_amount", "has_gems"): return L(lang, k + ".min.1", text, tgt, *tail)
            return L(lang, k + ".min", text, tgt, pct(lo), *tail)
        return L(lang, k + ".max", text, tgt, pct(hi), *tail)
    return L(lang, k, text, tgt)

def mult(lang, text, m, side):
    if none(m): return text
    t = m["type"].split(":")[1]
    k = "skill_bonus_multiplier.skilltree.%s.%s" % (t, side)
    if t == "attribute_value": return L(lang, k, text, fmt(m.get("divisor", 1)), attr_name(lang, m["attribute"]))
    if "item_condition" in m: return L(lang, k, text, item_name(lang, m["item_condition"], "where"))
    return L(lang, k, text)

def wrap(lang, text, b):
    text = mult(lang, text, b.get("enemy_multiplier"), "enemy")
    text = mult(lang, text, b.get("player_multiplier"), "player")
    text = living(lang, text, b.get("target_condition") or b.get("enemy_condition"), "target")
    return living(lang, text, b.get("player_condition"), "you")

def dmg_key(b):
    c = b.get("damage_condition")
    return "none" if none(c) else c["type"].split(":")[1]

def listener(lang, text, e):
    t = e["type"].split(":")[1]
    k = "event_listener.skilltree." + t
    if t == "item_used":
        text = L(lang, k, text, item_name(lang, e["item_condition"]))
    elif dmg_key(e) != "none":
        text = L(lang, k + ".damage", text, L(lang, "damage_condition.skilltree.%s.type" % dmg_key(e)))
    else:
        text = L(lang, k, text)
    return wrap(lang, text, e)

def secs(lang, value, ticks=False):
    s = value / 20 if ticks else value
    if s >= 60 and s % 60 == 0:
        return fmt(s / 60) + (" мин" if lang == "ru" else " min")
    return fmt(s) + (" сек" if lang == "ru" else " sec")

def item_bonus(lang, b):
    t = b["type"].split(":")[1]
    k = "item_bonus.skilltree." + t
    if t == "durability": return num(b["amount"], b["operation"], L(lang, k))
    if t in ("potion_duration", "food_saturation"): return num(b["multiplier"], 1, L(lang, k))
    if t == "potion_amplification": return num(b["chance"], 1, L(lang, k))
    if t == "quiver_capacity": return num(b["chance"], b["operation"], L(lang, k))
    if t == "food_healing": return L(lang, k, fmt(b["amount"]))
    if t == "sockets": return L(lang, k, b["amount"])
    if t == "food_effect":
        n = effect_name(lang, b["effect"])
        if b["effect"].startswith("skilltree:"):
            n = "+%s%% %s" % (fmt(b["amplifier"] + 1), n)
        return L(lang, k, n, secs(lang, b["duration"], True))
    if t == "skill_bonus": return bonus(lang, b["skill_bonus"])
    missing.add(k)
    return t

def bonus(lang, b):
    t = b["type"].split(":")[1]
    k = "skill_bonus.skilltree." + t
    if t == "attribute":
        return wrap(lang, num(b["amount"], b["operation"], attr_name(lang, b["attribute"])), b)
    if t == "damage":
        return wrap(lang, num(b["amount"], b["operation"], L(lang, "damage_condition.skilltree." + dmg_key(b))), b)
    if t in ("crit_chance", "crit_damage"):
        name = L(lang, k)
        if dmg_key(b) != "none":
            name += " (%s)" % L(lang, "damage_condition.skilltree." + dmg_key(b))
        return wrap(lang, num(b.get("chance", b.get("amount")), 1, name), b)
    if t in ("block_break_speed", "jump_height", "incoming_healing"):
        return wrap(lang, num(b["multiplier"], 1, L(lang, k)), b)
    if t == "arrow_retrieval": return num(b["chance"], 1, L(lang, k))
    if t == "gained_experience":
        return num(b["multiplier"], 1, L(lang, k, L(lang, "experience.source." + b["experience_source"])))
    if t == "loot_duplication":
        return num(b["chance"], 1, L(lang, k, fmt(b["multiplier"] * 100), L(lang, "loot.type." + b["loot_type"])))
    if t in ("healing", "ignite"):
        e = b["event_listener"]
        arg = fmt(b["amount"]) if t == "healing" else secs(lang, b["duration"])
        if b.get("chance", 1) < 1:
            text = num(b["chance"], 1, L(lang, "%s.%s.chance" % (k, e.get("target", "player")), arg))
        else:
            text = L(lang, "%s.%s" % (k, e.get("target", "player")), arg)
        return listener(lang, text, e)
    if t == "crafted_item_bonus":
        return L(lang, k, item_name(lang, b["item_condition"], "plural.type"), item_bonus(lang, b["item_bonus"]))
    if t == "repair_efficiency":
        return L(lang, k, item_name(lang, b["item_condition"], "plural.type"), num(b["multiplier"], 1, L(lang, k + ".bonus")))
    if t == "gem_power":
        return L(lang, k, item_name(lang, b["item_condition"], "where"), num(b["multiplier"], 1, L(lang, k + ".bonus")))
    if t == "player_sockets":
        return "+%s %s" % (b["sockets"], L(lang, k, item_name(lang, b["item_condition"], "where")))
    if t == "enchantment_amplification":
        c = b.get("enchantment_condition")
        ck = "none" if none(c) else c["type"].split(":")[1]
        return L(lang, k, L(lang, "enchantment_condition.skilltree." + ck), num(b["chance"], 1, L(lang, k + ".bonus")))
    if t == "enchantment_requirement": return L(lang, k, num(b["multiplier"], 1, L(lang, k + ".bonus")))
    if t == "free_enchantment": return L(lang, k, num(b["chance"], 1, L(lang, k + ".bonus")))
    if t == "recipe_unlock":
        rk = "recipe." + b["recipe_id"].replace(":", ".")
        return L(lang, k, L(lang, rk)) + " " + L(lang, rk + ".info", default="")
    missing.add(k)
    return t

# ---- effects the server understands (skilltree.rs: parse_tree) ---------------------------------------
def cond(c):
    """A player condition the server can check, None for none, False when it can't."""
    if none(c): return None
    t = c["type"].split(":")[1]
    if t in ("has_item_in_hand", "has_item_equipped"):
        ic = c["item_condition"]
        if ic["type"] == "skilltree:equipment_type":
            return {"k": "hand" if t == "has_item_in_hand" else "equip", "e": ic["equipment_type"]}
        return False
    if t == "health_percentage":
        return {"k": "hp", "min": c.get("min", -1), "max": c.get("max", -1)}
    return False

def effect(b):
    """The simplified effect, or None when this version doesn't run it (the line shows grey)."""
    t = b["type"].split(":")[1]
    if not none(b.get("player_multiplier")) or not none(b.get("enemy_multiplier")):
        return None
    if not none(b.get("target_condition")):
        return None
    c = cond(b.get("player_condition"))
    if c is False:
        return None
    if t == "attribute":
        a = b["attribute"]
        if a.startswith("minecraft:"):
            return {"t": "attr", "a": "minecraft:" + a.split(":")[1].replace("generic.", ""), "v": b["amount"], "op": b["operation"], "c": c}
        if a in ("skilltree:evasion", "skilltree:blocking", "skilltree:exp_per_minute") and b["operation"] == 0:
            return {"t": a.split(":")[1], "v": b["amount"], "c": c}
        return None
    if t == "damage":
        return {"t": "dmg", "v": b["amount"], "op": b["operation"], "dc": dmg_key(b), "c": c}
    if t == "crit_chance": return {"t": "crit", "v": b["chance"], "dc": dmg_key(b), "c": c}
    if t == "crit_damage": return {"t": "critdmg", "v": b["amount"], "dc": dmg_key(b), "c": c}
    if t == "block_break_speed": return {"t": "attr", "a": "minecraft:block_break_speed", "v": b["multiplier"], "op": 1, "c": c}
    if t == "jump_height": return {"t": "attr", "a": "minecraft:jump_strength", "v": b["multiplier"], "op": 1, "c": c}
    return None

# ---- atlas ----------------------------------------------------------------------------------------
atlas = Image.new("RGBA", (1024, 1024), (0, 0, 0, 0))
def tex(p): return Image.open(io.BytesIO(z.read("assets/skilltree/textures/" + p))).convert("RGBA")
icons = {}
def icon(path):
    name = path.split("/")[-1]
    if name not in icons:
        im = tex("icons/" + name)
        if im.size != (16, 16):
            im = im.resize((16, 16), Image.NEAREST)
        i = len(icons)
        atlas.paste(im, ((i % 64) * 16, (i // 64) * 16))
        icons[name] = i
    return icons[name]
FRAMES = ["lesser", "notable", "class", "gateway", "keystone"]
frames = {}
fx = 0
for n in FRAMES:  # three states side by side: the frame, the disc, the learned frame
    im = tex("icons/background/%s.png" % n)
    atlas.paste(im, (fx, 128))
    frames[n] = [fx, 128, im.size[0] // 3, im.size[1]]
    fx += im.size[0]
atlas.paste(tex("screen/skill_tree_background.png").resize((512, 512), Image.LANCZOS), (0, 200))

# ---- tree -----------------------------------------------------------------------------------------
ids = sorted(jload("data/skilltree/skill_trees/main_tree.json")["skillIds"])
raw = {i: jload("data/skilltree/skills/%s.json" % i.split(":")[1]) for i in ids}
index = {i: n for n, i in enumerate(ids)}
skills = []
total = run = 0
for i in ids:
    s = raw[i]
    short = i.split(":")[1]
    lines = {"ru": [], "en": []}
    fxs = []
    for b in s["bonuses"]:
        e = effect(b)
        total += 1
        run += e is not None
        if e is not None:
            fxs.append(e)
        for lang in ("ru", "en"):
            try:
                text = bonus(lang, b)
            except Exception as err:  # a shape this script doesn't know: name it, don't stop
                missing.add("%s: %r" % (b["type"], err))
                text = b["type"]
            lines[lang].append([" ".join(text.split()), 1 if e is not None else 0])
    for lang in ("ru", "en"):
        d = LANG[lang].get("skill.skilltree.%s.description" % short)
        if d:
            lines[lang].append([d, 2])
    skills.append({
        "id": short, "x": s["positionX"], "y": s["positionY"], "s": s["buttonSize"],
        "f": FRAMES.index(s["backgroundTexture"].split("/")[-1][:-4]), "i": icon(s["iconTexture"]),
        "st": 1 if s["isStartingPoint"] else 0,
        "d": [index[c] for c in s["directConnections"] if c in index],
        "l": [index[c] for c in s["longConnections"] if c in index],
        "o": [index[c] for c in s["oneWayConnections"] if c in index],
        "ru": [L("ru", "skill.skilltree.%s.name" % short)], "en": [L("en", "skill.skilltree.%s.name" % short)],
        "lru": lines["ru"], "len": lines["en"], "fx": fxs,
    })
atlas.save(out + "/atlas.png", optimize=True)
doc = {"frames": [frames[n] for n in FRAMES], "bg": [0, 200, 512, 512], "skills": skills}
with open(out + "/tree.json", "w", encoding="utf-8") as fh:
    json.dump(doc, fh, ensure_ascii=False, separators=(",", ":"))
print("skills", len(skills), "icons", len(icons), "bonuses", total, "run by the server", run)
if missing:
    print("MISSING", sorted(missing)[:40])
