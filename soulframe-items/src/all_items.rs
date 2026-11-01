//! Port of `generate_all_items.py`.
//!
//! Builds the unified Soulframe item catalogue by classifying every entity
//! in `packages_bin_attrs.json`, resolving display names via
//! `Languages.json`, cross-referencing `crafting_recipes.json` for
//! ingredients, synthesising one item per Aspect "Temper" from
//! `AspectManifest` entries, and de-duplicating overlapping store/raw
//! variants. The output schema and field semantics match the Python
//! original 1:1.

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

// ─── Input schemas ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PkgEntry {
    pub parent: String,
    pub attributes: String,
    // `path` is also stored in the value in the Python output but we always
    // key by the outer map, so we ignore it here.
    #[serde(default)]
    pub path: Option<String>,
}

pub type PkgAttrs = BTreeMap<String, PkgEntry>;
pub type Languages = HashMap<String, String>;

/// Load `Languages.json`, ignoring non-string entries such as `__order`.
fn load_languages(path: &Path) -> Result<Languages> {
    let raw: HashMap<String, Value> = serde_json::from_str(
        &fs::read_to_string(path).with_context(|| format!("Reading {:?}", path))?,
    )
    .context("Parsing Languages.json")?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
        .collect())
}

#[derive(Debug, Deserialize, Clone)]
pub struct RecipeIngredient {
    pub item: String,
    pub item_path: String,
    pub count: serde_json::Number,
    #[serde(default)]
    pub alternatives: Option<Value>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Recipe {
    pub path: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub rarity: Option<String>,
    #[serde(default)]
    pub result_item: Option<String>,
    #[serde(default)]
    pub ingredients: Vec<RecipeIngredient>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub localize_tag: Option<String>,
}

// ─── TemperRef ────────────────────────────────────────────────────────────

/// Lightweight reference to a single temper (aspect) that can randomly appear
/// on a weapon or other item that supports rolled aspects.
#[derive(Debug, Clone, Serialize)]
pub struct TemperRef {
    pub unique_name: String,
    pub name: String,
    /// Full lang tag for the display name, e.g. `/SF/Language/Upgrades/AspectRadiantDawnName`.
    pub lang_key: String,
    /// Full lang tag for the description, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang_desc_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

// ─── Internal item record ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub name: String,
    pub unique_name: String,
    pub path: String,
    pub lang_key: Option<String>,
    pub lang_desc_key: Option<String>,
    pub category: String,
    pub subcategory: String,
    pub rarity: Option<String>,
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crafting_recipe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingredients: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingredient_alternatives: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_manifest: Option<String>,
    /// Whether this item can be traded between players.
    /// `None` means the attribute was not present in the game data (trading
    /// may not yet be implemented for this item).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tradeable: Option<bool>,
    /// Virtue attributes referenced by this item's upgrade scripts.
    /// Populated from raw `AT_COURAGE`, `AT_SPIRIT`, and `AT_GRACE` markers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtues: Option<Vec<String>>,
    /// Tempers (aspects) that can randomly appear on this item when acquired.
    /// Populated when game data links an aspect manifest to this specific item
    /// or its weapon type.  `None` when no association is found in the data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub possible_tempers: Option<Vec<TemperRef>>,
    /// Whether a moonsteel (visually gilded / "shiny") variant of this item
    /// can be obtained.  `None` when the game data does not yet flag this.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_be_shiny: Option<bool>,
}

// ─── WeaponItem ────────────────────────────────────────────────────────────────

/// Value range used for charge-attack multipliers.
#[derive(Debug, Clone, Serialize)]
pub struct F64Range {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponItem {
    pub name: String,
    pub unique_name: String,
    pub path: String,
    pub lang_key: Option<String>,
    pub lang_desc_key: Option<String>,
    pub description: Option<String>,
    pub category: String,
    pub subcategory: String,
    pub rarity: Option<String>,
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crafting_recipe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingredients: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingredient_alternatives: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tradeable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_be_shiny: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtues: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub possible_tempers: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_manifest: Option<String>,
    pub grip_type: Option<String>,
    pub melee_style: Option<String>,
    pub product_category: Option<String>,
    pub damage: Option<f64>,
    pub damage_type: Option<String>,
    pub stability_damage: Option<f64>,
    pub fire_rate: Option<f64>,
    pub critical_hit_chance: Option<f64>,
    pub critical_hit_damage_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge_damage_multiplier: Option<F64Range>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge_stability_damage_multi: Option<F64Range>,
    pub perfect_charge_attack_range: Option<F64Range>,
    pub charge_armour_bypass_pct: Option<F64Range>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectile_amount: Option<f64>,
    pub requirements: Option<BTreeMap<String, i32>>,
    pub attunements: Option<BTreeMap<String, i32>>,
    pub attunements_cap: Option<BTreeMap<String, i32>>,
}

#[derive(Debug, Serialize)]
pub struct AvaApiCatalogue {
    metadata: AvaApiMetadata,
    items: Vec<AvaApiItem>,
    ingredient_index: BTreeMap<String, AvaApiIngredientIndexEntry>,
}

#[derive(Debug, Serialize)]
struct AvaApiMetadata {
    title: &'static str,
    description: &'static str,
    generated_at: String,
    source: &'static str,
    total_items: usize,
    categories: BTreeMap<String, usize>,
    total_unique_ingredients: usize,
}

/// Legacy all-items entry shape consumed by the AVA API.
///
/// This intentionally omits newer fields from [`Item`] such as `tradeable`,
/// `virtues`, and `can_be_shiny`. Weapon temper references are retained for
/// AVA consumers.
#[derive(Debug, Serialize)]
struct AvaApiItem {
    name: String,
    unique_name: String,
    path: String,
    lang_key: Option<String>,
    lang_desc_key: Option<String>,
    category: String,
    subcategory: String,
    rarity: Option<String>,
    icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crafting_recipe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ingredients: Option<Vec<AvaApiIngredient>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ingredient_alternatives: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    faction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_manifest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    possible_tempers: Option<Vec<TemperRef>>,
}

#[derive(Debug, Clone, Serialize)]
struct AvaApiIngredient {
    item: String,
    item_path: String,
    count: Value,
}

#[derive(Debug, Serialize)]
struct AvaApiIngredientIndexEntry {
    item_path: String,
    used_by: Vec<AvaApiIngredientUse>,
}

#[derive(Debug, Serialize)]
struct AvaApiIngredientUse {
    item: String,
    item_path: String,
    count: Value,
}

impl From<&Item> for AvaApiItem {
    fn from(item: &Item) -> Self {
        Self {
            name: item.name.clone(),
            unique_name: item.unique_name.clone(),
            path: item.path.clone(),
            lang_key: item.lang_key.clone(),
            lang_desc_key: item.lang_desc_key.clone(),
            category: item.category.clone(),
            subcategory: item.subcategory.clone(),
            rarity: item.rarity.clone(),
            icon: item.icon.clone(),
            crafting_recipe: item.crafting_recipe.clone(),
            ingredients: item.ingredients.as_deref().map(ava_api_ingredients),
            ingredient_alternatives: item.ingredient_alternatives.clone(),
            tag: item.tag.clone(),
            faction: item.faction.clone(),
            source_manifest: item.source_manifest.clone(),
            possible_tempers: item.possible_tempers.clone(),
        }
    }
}

fn ava_api_ingredient(value: &Value) -> AvaApiIngredient {
    AvaApiIngredient {
        item: value
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        item_path: value
            .get("item_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        count: value.get("count").cloned().unwrap_or(Value::Null),
    }
}

fn ava_api_ingredients(values: &[Value]) -> Vec<AvaApiIngredient> {
    values.iter().map(ava_api_ingredient).collect()
}

// ─── Base types ────────────────────────────────────────────────────────────

/// Unique-name set for entities that are base templates / non-items.
fn base_types() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        [
            // Armor base types
            "BaseBody",
            "BaseHelmet",
            "BaseLegs",
            "BaseSleeve",
            // Blueprint base types
            "BaseArmorBlueprint",
            "BaseWeaponBlueprint",
            "Fragment",
            "RecipeItem",
            "TierABaseBlueprint",
            "TierBBaseBlueprint",
            "TierCBaseBlueprint",
            "ColorPaletteBlueprint",
            "PactBlueprint",
            "PactFragment",
            "ArmorTokenBlueprint",
            // Currency/pickup internals
            "DracsIncreaseLargeItem",
            "DracsIncreaseModerateItem",
            "DracsIncreaseSmallItem",
            "DracsQuestRewardLargelItem",
            "DracsQuestRewardSmallItem",
            "DracsCraftingBundleItem",
            "DracsIncreaseLarge",
            "DracsIncreaseModerate",
            "DracsIncreaseSmall",
            "DracsQuestRewardsLargePickup",
            "DracsQuestRewardsSmallPickup",
            "ResourceManualToolUsePickupAction",
            // Cosmetic base types
            "Emote",
            "HolsterCustomization",
            "InstrumentDecoItem",
            "ShipDecoItem",
            "GuideBaseItem",
            "BaseVoice",
            "BaseHead",
            // Key Item base types
            "BaseKeyItem",
            "BaseKeyPickup",
            "BaseTransientKeyItem",
            "SFKeyItem",
            // Kintsugi base types
            "BaseKintsugiArmourHelmUpgrade",
            "BaseKintsugiArmourLegsUpgrade",
            "BaseKintsugiArmourTorsoUpgrade",
            "BaseKintsugiArmourUpgrade",
            "BaseKintsugiUniversalUpgrade",
            "BaseKintsugiUpgrade",
            "BaseKintsugiWeaponUpgrade",
            "BaseKintsugiWeaponUpgradeCaster",
            "BaseKintsugiWeaponUpgradeRanged",
            // Misc base types
            "EmailItem",
            "MiscItemBase",
            "FocusLens",
            "LeagueTicketItem",
            "ColourPickerAsset",
            "FishItem",
            "FusionBundle",
            "GrowingPlant",
            "ShipModuleItem",
            "TransientMiscItem",
            "TransmogTokenBase",
            // Helmet votive variants (base templates, not inventory items)
            "BaseFemaleHead",
            "BaseMaleHead",
            "BaseHelmetVotive",
            "BaseHelmetVotiveNoHair",
            // Mote/Upgrade base types
            "BasePactUpgrade",
            "SFMotesBaseUpgrade",
            "SFWeaponUpgrade",
            "SFPactUpgrade",
            // Resource base types
            "OrganicBaseResourceItem",
            "ResourceItem",
            "ResourceManualPickup",
            "ResourcePickup",
            // Soul base types
            "BaseSoul",
            // Store base types
            "SFSuitCustomization",
            "SFVoiceCustomization",
            // Totem base types
            "TotemBaseItem",
            // Pact/Idol base types
            "SFIdolMod",
            // Pact category base types
            "ArcanicPactUpgrade",
            "InstinctPactUpgrade",
            "VirtuePactUpgrade",
            // Child head variants
            "ChildHeadA",
            "ChildHeadB",
            "ChildHeadC",
            "ChildHeadD",
            "ChildHeadE",
            // Forget Fire head variants
            "ForgetFire1Head",
            "ForgetFire2Head",
            "ForgetFire3Head",
            "ForgetFire4Head",
            "ForgetFire5Head",
            // Hair base types
            "Hair2AHelmet",
            "Hair3AHelmet",
            // Internal gameplay
            "FusionTreasure",
            "PlayAnimalFlute",
            // Quest internal assets
            "BromiusAcornMeleeTree",
        ]
        .into_iter()
        .collect()
    })
}

// ─── Lazy compiled regexes ─────────────────────────────────────────────────

fn re(pattern: &'static str) -> &'static Regex {
    use std::collections::HashMap as Map;
    use std::sync::Mutex;
    static CACHE: OnceLock<Mutex<Map<&'static str, &'static Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Map::new()));
    let mut guard = cache.lock().unwrap();
    if let Some(r) = guard.get(pattern) {
        return r;
    }
    let r: &'static Regex = Box::leak(Box::new(Regex::new(pattern).expect("valid regex")));
    guard.insert(pattern, r);
    r
}

fn capture_after<'a>(text: &'a str, pat: &'static str) -> Option<&'a str> {
    re(pat)
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str())
}

fn parse_tradeable(attrs: &str) -> Option<bool> {
    capture_after(attrs, r"Tradeable=(\w+)")
        .or_else(|| capture_after(attrs, r"IsTradeable=(\w+)"))
        .or_else(|| capture_after(attrs, r"Tradable=(\w+)"))
        .map(|v| matches!(v, "true" | "True" | "1" | "yes"))
        .or_else(|| {
            capture_after(attrs, r"TradeCapability=(\w+)").and_then(|v| match v {
                "RO_ALWAYS" => Some(true),
                "RO_NEVER" => Some(false),
                _ => None,
            })
        })
}

fn is_mote_lang_tag(tag: &str) -> bool {
    tag.starts_with("/SF/Language/Motes/")
}

fn has_mote_lang_tag(attrs: &str) -> bool {
    capture_after(attrs, r"LocalizeTag=(/[\w/]+)").map_or(false, is_mote_lang_tag)
        || capture_after(attrs, r"LocalizeDescTag=(/[\w/]+)").map_or(false, is_mote_lang_tag)
}

fn mote_subcategory(path: &str) -> &'static str {
    let pairs: &[(&str, &str)] = &[
        ("/Bow/", "Bow"),
        ("/Daggers/", "Dagger"),
        ("/FlyBlades/", "Flyblade"),
        ("/GreatSword/", "Greatsword"),
        ("/LongBlade/", "Long Blade"),
        ("/Polearms/", "Polearm"),
        ("/Shield/", "Shield"),
        ("/Magiks/", "Magick"),
    ];
    for (tag, label) in pairs {
        if path.contains(tag) {
            return label;
        }
    }
    "General"
}

fn parse_virtues(attrs: &str) -> Option<Vec<String>> {
    let virtues: Vec<String> = [
        ("AT_COURAGE", "courage"),
        ("AT_SPIRIT", "spirit"),
        ("AT_GRACE", "grace"),
    ]
    .into_iter()
    .filter_map(|(raw, value)| attrs.contains(raw).then(|| value.to_string()))
    .collect();

    (!virtues.is_empty()).then_some(virtues)
}

fn resolve_mote_virtues(path: &str, attrs: &str, pkg_attrs: &PkgAttrs) -> Option<Vec<String>> {
    if let Some(type_name) = capture_after(attrs, r"TypeName=(/[\w/]+)") {
        if let Some(entry) = pkg_attrs.get(type_name) {
            return resolve_mote_virtues(type_name, entry.attributes.as_str(), pkg_attrs);
        }
    }

    let has_player_facing_name = capture_after(attrs, r"LocalizeTag=(/[\w/]+)").is_some();
    if !has_player_facing_name {
        return None;
    }

    let utility_upgrade = capture_after(attrs, r"_utilityUpgrade=([\w]+)")?;
    let dir = path.rsplit_once('/').map(|(dir, _)| dir)?;
    let utility_path = format!("{dir}/{utility_upgrade}");
    pkg_attrs
        .get(&utility_path)
        .and_then(|entry| parse_virtues(entry.attributes.as_str()))
}

fn resolve_item_virtues(
    category: &str,
    path: &str,
    attrs: &str,
    pkg_attrs: &PkgAttrs,
) -> Option<Vec<String>> {
    if category == "Mote" {
        resolve_mote_virtues(path, attrs, pkg_attrs)
    } else {
        parse_virtues(attrs)
    }
}

// ─── classify_item ─────────────────────────────────────────────────────────

/// Determine `(category, subcategory)` for an entity. Mirrors the Python
/// `classify_item` exactly: same path-substring checks, in the same order.
pub fn classify_item(path: &str, parent: &str, attrs: &str) -> (&'static str, &'static str) {
    let uname = path.rsplit('/').next().unwrap_or("");

    // Base type / template definitions
    if base_types().contains(uname) {
        return ("_SKIP", "Base Type");
    }

    // Enemy / NPC paths
    if path.contains("/Types/Enemies/") {
        return ("_SKIP", "Enemy");
    }
    if path.contains("/StoreItems/Types/Enemies/") {
        return ("_SKIP", "Enemy Store Item");
    }
    if path.contains("/Types/NeutralPeople/") {
        return ("_SKIP", "NPC");
    }
    if path.contains("/Souls/Wolf/") && path.contains("MeleeWeapon") {
        return ("_SKIP", "Soul Internal Weapon");
    }
    if path.contains("/Types/Items/Aspects/") && path.ends_with("AspectManifest") {
        return ("_SKIP", "Aspect Manifest Container");
    }
    if path.contains("/Souls/Fenn/") && path.contains("MeleeWeapon") {
        return ("_SKIP", "Soul Internal Weapon");
    }
    if path.contains("/Types/GameRules/") {
        return ("_SKIP", "Game Rules");
    }
    if path.contains("/Types/Challenges/") {
        return ("_SKIP", "Challenge Definition");
    }
    if path.contains("/Types/Player/") && !path.contains("/StoreItems/") {
        return ("_SKIP", "Player Definition");
    }
    if path == "/SF/Souls/Default/InnateUpgrade" {
        return ("_SKIP", "Legacy Artifact Upgrade");
    }

    if path == "/SF/StoreItems/Types/Game/SFWeapon"
        || path == "/SF/StoreItems/Types/Game/SFMeleeWeapon"
    {
        return ("_SKIP", "Base Weapon Type");
    }
    if path == "/SF/StoreItems/Types/Game/SFSkillTree" {
        return ("_SKIP", "Skill Tree Store Wrapper");
    }
    if path.contains("/StoreItems/Weapons/Tenno/") {
        return ("_SKIP", "Legacy Tenno Weapon");
    }

    if path.contains("/Types/Quests/") && uname.contains("Weapon") {
        return ("_SKIP", "Quest Asset Weapon");
    }
    if uname.contains("Corrupted")
        && (uname.contains("MeleeWeapon") || uname.contains("RangedWeapon"))
        && !path.contains("/StoreItems/")
    {
        return ("_SKIP", "Enemy Weapon");
    }
    if uname.contains("AIMeleeWeapon") || uname.contains("GateGuard") {
        return ("_SKIP", "AI Weapon");
    }
    if path.contains("SinecureKnight") && uname.contains("Weapon") && !path.contains("/StoreItems/")
    {
        return ("_SKIP", "NPC Weapon");
    }
    if path.contains("MendicantKnight")
        && uname.contains("Weapon")
        && !path.contains("/StoreItems/")
    {
        return ("_SKIP", "NPC Weapon");
    }
    if path.contains("NimrodSpear") && !path.contains("/StoreItems/") {
        return ("_SKIP", "NPC Weapon");
    }
    if path.contains("OdeSpikedMace") && !path.contains("/StoreItems/") {
        return ("_SKIP", "NPC Weapon");
    }
    if path.contains("/Grenades/") && uname.contains("RangedWeapon") {
        return ("_SKIP", "Grenade (NPC)");
    }
    if uname == "ThrowingKnifeRangedWeapon" {
        return ("_SKIP", "Generic Thrown Weapon");
    }
    if path.contains("EnvoyHunterBow") {
        return ("_SKIP", "NPC Weapon");
    }
    if uname.contains("TorchPickUpMeleeWeapon") || uname.contains("TorchOffPickUpMeleeWeapon") {
        return ("_SKIP", "Torch Pickup");
    }
    if uname == "GraceUpgrade" && path.contains("/Weapons/Swords/") {
        return ("_SKIP", "Internal Upgrade");
    }

    // Skip resource/asset file paths (texture maps, meshes, sounds) that share a
    // game-data namespace with gameplay entities but are not inventory items.
    if uname.contains('.') {
        return ("_SKIP", "Resource File");
    }
    // Surface-effect containers are engine material/physics entities, not items.
    if uname.contains("SurfaceEffectContainer") {
        return ("_SKIP", "Surface Effect Container");
    }

    // Most player-accessible weapons live under /StoreItems/Weapons/. Some newer
    // weapons are stored directly at /SF/Weapons/ with MarketMode=MM_VISIBLE
    // (e.g. SeedRapierMeleeWeapon, WazzardStaffCasterRangedMeleeWeapon).
    // Only skip /SF/Weapons/ entries that are NOT market-visible; those that are
    // visible in the market will fall through to the weapon classifiers below.
    if path.contains("/SF/Weapons/") && !attrs.contains("MarketMode=MM_VISIBLE") {
        return ("_SKIP", "Weapon Internal Entity");
    }

    // Fenn Starter Motes (early catch)
    if path.contains("/Upgrades/FennStarterMotes/") {
        return ("Mote", "Starter Mote");
    }

    // Player vs AI weapon disambiguation in /Weapons/Melee/
    if path.contains("/Weapons/Melee/") && !path.contains("/StoreItems/") {
        if let Some(tag) = capture_after(attrs, r"LocalizeTag=(/[\w/]+)") {
            if tag == "/Lotus/Language/Menu/Loadout_AnyWeapon" {
                return ("_SKIP", "Generic Weapon Base Type");
            }
        }
    }

    // Dendria words misfiled under Weapons
    if path.contains("/Upgrades/Dendria/") {
        let sub = if path.contains("Weapon") {
            "Weapon Word"
        } else {
            "Pact Word"
        };
        return ("Dendria Word", sub);
    }

    // Reforge blueprints (duplicates)
    if path.contains("Reforge")
        && (path.contains("/Recipes/")
            || parent.contains("RecipeStoreItem")
            || parent.contains("RecipeItem"))
    {
        return ("_SKIP", "Reforge Blueprint");
    }

    // Recipes / Blueprints
    if path.contains("/Recipes/")
        || parent.contains("RecipeStoreItem")
        || parent.contains("RecipeItem")
    {
        let pairs: &[(&str, &str)] = &[
            ("/Armor/", "Armor"),
            ("/Weapons/", "Weapons"),
            ("/Melee/", "Weapons"),
            ("/Consumables/", "Consumables"),
            ("/Consumeables/", "Consumables"),
            ("/Totems/", "Totems"),
            ("/Companions/", "Companions"),
            ("/Resources/", "Resources"),
            ("/Keys/", "Keys"),
            ("/Dojo/", "Dojo"),
            ("/Skins/", "Skins"),
            ("/Sigils/", "Sigils"),
            ("/Syandanas/", "Syandanas"),
            ("/ColorPalettes/", "Color Palettes"),
            ("/Customization/", "Customization"),
            ("/Elixers/", "Elixirs"),
            ("/Joinery/", "Joinery"),
            ("/Pacts/", "Pacts"),
            ("/Holiday/", "Holiday"),
            ("/Idols/", "Idols"),
            ("/Fragment/", "Fragments"),
        ];
        let mut sub = "Other";
        for (tag, label) in pairs {
            if path.contains(tag) {
                sub = label;
                break;
            }
        }
        return ("Blueprint", sub);
    }

    // Weapons (actual, not blueprints)
    // Exclude SFWeaponAttachment parents (visual mesh attachments, not items)
    // and Soul-path entities (NPC/soul internal weapons resolved later in the
    // /Souls/ catch-all below).
    if [
        "SFWeaponStoreItem",
        "SFWeapon",
        "SFMeleeWeapon",
        "SFRangedMeleeWeapon",
        "SFCasterMeleeWeapon",
    ]
    .iter()
    .any(|k| parent.contains(k))
        && !parent.contains("Attachment")
        && !path.contains("/Souls/")
    {
        if let Some(tag) = capture_after(attrs, r"LocalizeTag=(/[\w/]+)") {
            if tag == "/Lotus/Language/Menu/Loadout_AnyWeapon" {
                return ("_SKIP", "Generic Weapon Base Type");
            }
        }
        let sub = if path.contains("Ranged") || path.contains("Bow") || path.contains("Caster") {
            "Ranged"
        } else {
            "Melee"
        };
        return ("Weapon", sub);
    }

    if path.contains("/StoreItems/Weapons/") {
        if let Some(tag) = capture_after(attrs, r"LocalizeTag=(/[\w/]+)") {
            if tag == "/Lotus/Language/Menu/Loadout_AnyWeapon" {
                return ("_SKIP", "Generic Weapon Base Type");
            }
        }
        let sub = if path.contains("Ranged") {
            "Ranged"
        } else {
            "Melee"
        };
        return ("Weapon", sub);
    }

    // Armor (skins/pieces)
    if path.contains("/Upgrades/Skins/Helmets/")
        || path.contains("/Upgrades/Skins/Body/")
        || path.contains("/Upgrades/Skins/Legs/")
        || path.contains("/Upgrades/Skins/Sleeve/")
    {
        let slot = if path.contains("/Helmets/") {
            "Helmet"
        } else if path.contains("/Body/") {
            "Body"
        } else if path.contains("/Legs/") {
            "Legs"
        } else if path.contains("/Sleeve/") {
            "Sleeve"
        } else {
            "Other"
        };
        return ("Armor", slot);
    }
    if path.contains("/Upgrades/Skins/Head/") {
        return ("Cosmetic", "Head Customization");
    }
    if path.contains("/Upgrades/Skins/Totem/") || path.contains("/Skins/Totem/") {
        return ("Totem", "Totem");
    }
    if has_mote_lang_tag(attrs) {
        return ("Mote", mote_subcategory(path));
    }
    if path.contains("/Upgrades/Totems/") {
        return ("Totem", "Totem Upgrade");
    }
    if path.contains("/Upgrades/Boons/") && uname.contains("BoonItemBundle") {
        return ("Bundle", "Boon Bundle");
    }

    // Motes
    if path.contains("/Upgrades/Motes/")
        || parent.contains("SFArtifactUpgrade")
        || parent.contains("BaseArtifactUpgrade")
    {
        return ("Mote", mote_subcategory(path));
    }

    // Runes
    if path.contains("/Upgrades/Runes/")
        || parent.contains("SFRunesMajor")
        || parent.contains("SFRunesMinor")
    {
        return ("Rune", "Weapon Rune");
    }

    // Dendria (again — order-equivalent to Python)
    if path.contains("/Upgrades/Dendria/") {
        let sub = if path.contains("Weapon") {
            "Weapon Word"
        } else {
            "Pact Word"
        };
        return ("Dendria Word", sub);
    }

    // Techniques
    if path.contains("/Upgrades/Techniques/") || parent.contains("SFSkillUpgrade") {
        let pairs: &[(&str, &str)] = &[
            ("/Archery/", "Archery"),
            ("/Casting/", "Magick"),
            ("/Dagger/", "Dagger"),
            ("/GreatSword/", "Greatsword"),
            ("/Polearm/", "Polearm"),
            ("/Shield/", "Shield"),
            ("/Sword/", "Long Blade"),
            ("/Throwing/", "Flyblade"),
            ("/Pacts/", "Pact"),
        ];
        let mut sub = "General";
        for (tag, label) in pairs {
            if path.contains(tag) {
                sub = label;
                break;
            }
        }
        return ("Technique", sub);
    }

    // Resources
    if path.contains("/Types/PickUps/Resources/") {
        let mut sub: &'static str = "Other";
        let parts: Vec<&str> = path.split('/').collect();
        for (i, p) in parts.iter().enumerate() {
            if *p == "Resources" && i + 1 < parts.len() {
                let np = parts[i + 1];
                if !np.ends_with("Resource") && !np.ends_with("Item") {
                    // Leak the slice as static — these are small + finite.
                    sub = Box::leak(np.to_string().into_boxed_str());
                }
                break;
            }
        }
        return ("Resource", sub);
    }

    if path.contains("/Types/PickUps/Currencies/") || path.contains("/Currency/") {
        return ("Currency", "Currency");
    }
    if path.contains("/Types/PickUps/Keys/")
        || parent.contains("BaseKeyPickup")
        || parent.contains("BaseKeyItem")
    {
        return ("Key Item", "Key");
    }
    if path.contains("/Types/Items/Consumables/") {
        return ("Consumable", "Elixir");
    }
    if path.contains("/Types/Items/ColorPalettes/") || path.contains("ColorPalette") {
        return ("Cosmetic", "Color Palette");
    }
    if path.contains("/Types/Items/Kintsugi/") {
        return ("Kintsugi", "Weapon Upgrade Material");
    }
    if path.contains("/Types/Items/TransmogTokens/") {
        return ("Cosmetic", "Transmog Token");
    }
    if path.contains("/Types/Items/Emotes/") {
        return ("Cosmetic", "Emote");
    }
    if path.contains("/Types/StoreItems/AvatarImages/") || parent.contains("AvatarImageItem") {
        return ("Cosmetic", "Avatar Image");
    }
    if path.contains("/Upgrades/Skins/Sparrow/") {
        return ("Cosmetic", "Sparrow Skin");
    }
    if path.contains("/Upgrades/Skins/Mother/") {
        return ("Cosmetic", "Mother Skin");
    }
    if path.contains("/Upgrades/Skins/Voice/") {
        return ("Cosmetic", "Voice");
    }
    if path.contains("/Upgrades/Skins/Hair/") {
        return ("Cosmetic", "Hair Style");
    }
    if path.contains("/Types/Items/AttributeItems/") {
        return ("Consumable", "Attribute Item");
    }
    if path.contains("/Types/Items/AncestorItems/") {
        return ("Key Item", "Ancestor Item");
    }
    if path.contains("/Types/Items/FactionItems/") {
        return ("Key Item", "Faction Item");
    }
    if path.contains("/StoreItems/Souls/") {
        return ("Soul", "Ancestor Soul");
    }
    // Exclude /StoreItems/Types/Game/ paths: those are AutoStoreItem wrappers
    // that may have "Booster" in their entity name for legacy reasons but are
    // actually other item types (e.g. HolsterCustomization).
    if path.contains("Booster")
        && parent.contains("StoreItem")
        && !path.contains("/StoreItems/Types/Game/")
    {
        return ("Booster", "Booster");
    }
    if path.contains("/StoreItems/Types/Game/")
        && uname.contains("Booster")
        && capture_after(attrs, r"LocalizeTag=(/[\w/]+)")
            .is_some_and(|tag| tag.contains("/Boosters/"))
    {
        return ("Booster", "Booster");
    }
    // StoreItem quest rewards must be caught before the generic /Types/Quests/ check
    // below, because /StoreItems/Types/Quests/ is a substring of /Types/Quests/.
    if path.contains("/StoreItems/Types/Quests/") {
        if path.contains("QuestAssets") {
            return ("Quest Item", "Quest");
        }
        return ("Quest", "Quest");
    }
    if path.contains("/Types/Quests/") && path.contains("QuestAssets") {
        return ("Quest Item", "Quest");
    }
    if path.contains("/StoreItems/ArmorBundles/") || path.contains("/StoreItems/Packages/") {
        return ("Bundle", "Bundle");
    }
    if path.contains("/StoreItems/PactBundles/") {
        return ("Bundle", "Pact Bundle");
    }
    if path.contains("/StoreItems/DracBundles/") {
        return ("Bundle", "Currency Bundle");
    }
    if path.contains("/StoreItems/SeasonalBundle") {
        return ("Bundle", "Seasonal Bundle");
    }
    if path.contains("/Types/Items/MiscItems/") {
        return ("Misc Item", "Misc");
    }
    if path.contains("/Types/StoreItems/SlotItems/") || parent.contains("SlotItem") {
        return ("Slot Item", "Inventory Slot");
    }
    if path.contains("/Types/Items/Fish/") {
        return ("Resource", "Fish");
    }
    if parent.contains("ShipItem") || parent.contains("NavigationFeatureItem") {
        return ("Misc Item", "Ship");
    }
    if path.contains("/Types/Quests/") {
        return ("Quest", "Quest");
    }

    // Actual weapon type definitions
    if re(r"^/SF/Weapons/(Melee|Ranged|Bow|Daggers|Hammers|Axes|Maces|Shields|Spear|Swords|Staves|CastingDevice|Guns|ShoulderCannons|MusicalInstruments)/")
        .is_match(path)
        && !path.contains("/MeleeTrees/")
        && !path.contains("/MeleeAttackSets/")
    {
        let sub = if ["Ranged", "Bow", "CastingDevice", "Guns", "ShoulderCannons"]
            .iter()
            .any(|k| path.contains(k))
        {
            "Ranged"
        } else {
            "Melee"
        };
        return ("Weapon", sub);
    }

    if path.contains("/Weapons/MeleeTrees/") || path.contains("/MeleeAttackSets/") {
        return ("_SKIP", "Melee Tree");
    }

    if path.contains("/Souls/") && uname.contains("Ability") {
        return ("_SKIP", "Soul Ability");
    }
    if path.contains("/Souls/") && uname.contains("Projectile") {
        return ("_SKIP", "Soul Projectile");
    }
    if path.contains("/Souls/") && uname.contains("Trigger") {
        return ("_SKIP", "Soul Trigger");
    }
    if path.contains("/Souls/") && uname.contains("Attachment") {
        return ("_SKIP", "Soul Attachment");
    }
    if path.contains("/Souls/") && uname.contains("PactUpgrade") {
        return ("Pact Upgrade", "Pact Node");
    }
    if path.contains("/Souls/") && uname.contains("Soul") {
        return ("Soul", "Soul Definition");
    }
    if path.contains("/Souls/") && uname.contains("IdolMod") {
        return ("Pact Upgrade", "Idol Mod");
    }

    if path.contains("/Souls/Common/Virtues/") {
        return ("Virtue", "Virtue Stone");
    }
    if path.contains("/Upgrades/FennStarterMotes/") {
        return ("Mote", "Starter Mote");
    }

    if path.contains("/Types/Enemies/") {
        return ("_SKIP", "Enemy");
    }
    if path.contains("/Types/NeutralPeople/") {
        return ("_SKIP", "NPC");
    }
    // Engine base type definitions at /Types/Game/ that are not StoreItems.
    // /StoreItems/Types/Game/ is caught earlier as ("Store Item", "Game Item").
    // Must precede uname.contains("Booster") to prevent /SF/Types/Game/GraceBooster
    // from leaking through as a Booster item.
    if path.contains("/Types/Game/") && !path.contains("/StoreItems/") {
        return ("_SKIP", "Game Type Definition");
    }
    if uname.contains("Booster") && !path.contains("/StoreItems/Types/Game/") {
        return ("Booster", "Booster");
    }
    if path.contains("/Types/Items/ShipDecos/") {
        return ("Cosmetic", "Ship Decoration");
    }
    if path.contains("/Types/Items/EmailItems/") {
        return ("Misc Item", "Email Reward");
    }
    if path.contains("/Types/Items/LeagueTicketItems/") {
        return ("Misc Item", "League Ticket");
    }
    if path.contains("/Types/Items/FusionTreasure") {
        return ("Misc Item", "Fusion Treasure");
    }

    // Engine base templates like /SF/Types/Items/XxxItem (no subfolder)
    if re(r"^/SF/Types/Items/\w+Item$").is_match(path) {
        return ("_SKIP", "Base Type Definition");
    }

    if path.contains("HolsterCustomization") || path.contains("HolsterInfo") {
        return ("Cosmetic", "Holster Customization");
    }
    if path.contains("/Types/Keys/") {
        return ("Key Item", "Key");
    }

    if path.contains("/StoreItems/Types/Items/") {
        let pairs: &[(&str, &str)] = &[
            ("/Kintsugi/", "Kintsugi"),
            ("/TransmogTokens/", "Transmog Token"),
            ("/Consumables/", "Consumable"),
            ("/AttributeItems/", "Attribute Item"),
            ("/ColorPalettes/", "Color Palette"),
            ("/Emotes/", "Emote"),
        ];
        let mut sub = "General";
        for (tag, label) in pairs {
            if path.contains(tag) {
                sub = label;
                break;
            }
        }
        return ("Store Item", sub);
    }
    if path.contains("/StoreItems/Types/PickUps/") {
        return ("Store Item", "Pickup");
    }
    if path.contains("/StoreItems/Types/StoreItems/") {
        return ("Store Item", "Store Item");
    }
    if path.contains("/StoreItems/Types/Game/") {
        return ("Store Item", "Game Item");
    }
    if path.contains("/StoreItems/Types/Gameplay/") {
        return ("Store Item", "Gameplay Item");
    }

    if path.contains("/StoreItems/Upgrades/Motes/") {
        return ("Mote", "Store Mote");
    }
    if path.contains("/StoreItems/Upgrades/Runes/") {
        return ("Rune", "Store Rune");
    }
    if path.contains("/StoreItems/Upgrades/Dendria/") {
        return ("Dendria Word", "Store Dendria Word");
    }
    if path.contains("/StoreItems/Upgrades/Techniques/") {
        return ("Technique", "Store Technique");
    }
    if path.contains("/StoreItems/Upgrades/FennStarterMotes/") {
        return ("Mote", "Starter Mote");
    }
    if path.contains("/StoreItems/Upgrades/Skins/") {
        let pairs: &[(&str, &str)] = &[
            ("/Helmets/", "Helmet"),
            ("/Body/", "Body"),
            ("/Legs/", "Legs"),
            ("/Sleeve/", "Sleeve"),
            ("/Totem/", "Totem"),
            ("/Head/", "Head"),
            ("/Hair/", "Hair Style"),
            ("/Sparrow/", "Sparrow Skin"),
            ("/Mother/", "Mother Skin"),
            ("/Voice/", "Voice"),
        ];
        let mut sub = "Other";
        for (tag, label) in pairs {
            if path.contains(tag) {
                sub = label;
                break;
            }
        }
        return ("Store Item", sub);
    }

    if parent.contains("SFSuitCustomizationStoreItem") {
        return ("Store Item", "Armor Piece");
    }
    if parent.contains("AutoStoreItem") {
        return ("Store Item", "Auto Store Item");
    }
    if path.contains("/Syndicates/") {
        return ("Syndicate", "Syndicate");
    }
    if path.contains("/Types/PickUps/Motes/") {
        return ("Mote", "World Mote Pickup");
    }
    if path.contains("/Types/PickUps/DropTables/") || parent.contains("SFDropTable") {
        return ("Drop Table", "Drop Table");
    }
    if path.contains("/Types/PickUps/") {
        return ("Pickup", "Misc Pickup");
    }
    if parent.contains("MiscItemBaseStoreItem") {
        return ("Misc Item", "Store Misc");
    }
    if path.contains("/Types/Gameplay/") {
        return ("_SKIP", "Gameplay System");
    }
    if parent.contains("PowerSuit") || path.contains("/Types/Player/") {
        return ("_SKIP", "Player Definition");
    }
    if path.contains("/Types/Game/") {
        return ("_SKIP", "Game Type Definition");
    }
    if path.contains("/Weapons/Ammo/") {
        return ("_SKIP", "Ammo Pickup");
    }
    if path.contains("/Sounds/") {
        return ("_SKIP", "Sound");
    }
    if path.contains("/Weapons/OneOffWeapons/") {
        return ("_SKIP", "One-Off Weapon");
    }
    if path.contains("/Types/MeleeTree/") {
        return ("_SKIP", "Melee Tree Definition");
    }
    if path.starts_with("/SF/Upgrades/SF") {
        return ("_SKIP", "Upgrade Base Type");
    }
    if path.contains("/Souls/") {
        if uname.contains("MeleeWeapon") || uname.contains("Deco") {
            return ("_SKIP", "Soul Internal");
        }
        if uname.contains("AbilityCards") {
            return ("Misc Item", "Ability Card");
        }
        return ("_SKIP", "Soul Internal");
    }

    if path.starts_with("/Lotus/") {
        if path.contains("/StoreItems/") || path.contains("/Types/Items/") {
            return ("Legacy Item", "Warframe Legacy");
        }
        if path.contains("/Upgrades/Mods/") {
            return ("Legacy Item", "Mod (Legacy)");
        }
        if path.contains("/Upgrades/Skins/") {
            return ("Legacy Item", "Skin (Legacy)");
        }
        if path.contains("/Types/PickUps/") {
            return ("Legacy Item", "Pickup (Legacy)");
        }
        if path.contains("/Types/StoreItems/") {
            return ("Legacy Item", "Store (Legacy)");
        }
        if path.contains("/Syndicates/") {
            return ("Legacy Item", "Syndicate (Legacy)");
        }
        if path.contains("/Upgrades/") {
            return ("Legacy Item", "Upgrade (Legacy)");
        }
        return ("Legacy Item", "Other (Legacy)");
    }

    ("Uncategorized", "Other")
}

// ─── filters ──────────────────────────────────────────────────────────────

fn is_item_entity(path: &str, parent: &str, attrs: &str) -> bool {
    if !(path.starts_with("/SF/") || path.starts_with("/Lotus/")) {
        return false;
    }
    if capture_after(attrs, r"LocalizeTag=(/[\w/]+)").is_some() {
        return true;
    }
    let item_path_markers = [
        "/StoreItems/",
        "/Types/Recipes/",
        "/Types/PickUps/Resources/",
        "/Types/PickUps/Currencies/",
        "/Types/PickUps/Keys/",
        "/Types/Items/",
        "/Types/StoreItems/",
    ];
    if item_path_markers.iter().any(|m| path.contains(m)) {
        return true;
    }
    let item_parent_markers = [
        "StoreItem",
        "RecipeItem",
        "PickUpItem",
        "ResourceItem",
        "ResourcePickup",
        "ResourceManualPickup",
        "KeyPickup",
        "KeyItem",
        "ArtifactUpgrade",
        "WeaponUpgrade",
        "SkillUpgrade",
        "AvatarImageItem",
        "SlotItem",
    ];
    item_parent_markers.iter().any(|m| parent.contains(m))
}

fn is_player_facing_sparrow_skin(path: &str, attrs: &str) -> bool {
    let Some(relative_path) = path.strip_prefix("/SF/Upgrades/Skins/Sparrow/") else {
        return false;
    };

    !relative_path.contains('/')
        && capture_after(attrs, r"LocalizeTag=(/[\w/]+)").is_some()
        && capture_after(attrs, r"Icon=(/[\w/.]+)").is_some()
}

fn is_soulframe_relevant(item: &Item) -> bool {
    if item.path.starts_with("/SF/") {
        return true;
    }
    if let Some(lk) = item.lang_key.as_deref() {
        if lk.starts_with("/SF/") {
            return true;
        }
    }
    false
}

// ─── tempers ──────────────────────────────────────────────────────────────

fn split_top_level_braces(block: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut start: Option<usize> = None;
    for (i, ch) in block.char_indices() {
        match ch {
            '{' => {
                if depth == 0 {
                    start = Some(i + 1);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start.take() {
                        out.push(block[s..i].to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn temper_compat_label(compat: &str) -> &'static str {
    match compat {
        "/SF/Types/Game/SFMeleeWeapon" => "Melee Weapon",
        "/SF/Types/Game/SFRangedWeapon" => "Ranged Weapon",
        "/SF/Types/Game/SFWeapon" => "Weapon",
        _ => "Weapon",
    }
}

struct TemperChunk {
    manifest_path: String,
    compat: String,
    subcategory: &'static str,
    prefix: String,
    suffix: Option<String>,
    tag: Option<String>,
    icon: Option<String>,
    faction: Option<String>,
    is_reference_only: bool,
}

fn extract_tempers(
    pkg_attrs: &PkgAttrs,
    lang: &Languages,
) -> (
    Vec<Item>,
    HashMap<String, Vec<TemperRef>>, // by_compat:    ItemCompatibility value → tempers
    HashMap<String, Vec<TemperRef>>, // by_manifest:  manifest path          → tempers
) {
    let mut chunks: Vec<TemperChunk> = Vec::new();

    for (manifest_path, entry) in pkg_attrs {
        if !manifest_path.contains("/Types/Items/Aspects/")
            || !manifest_path.ends_with("AspectManifest")
        {
            continue;
        }
        let uname = manifest_path.rsplit('/').next().unwrap_or("");
        if uname.starts_with("Test") {
            continue;
        }
        let attrs = entry.attributes.as_str();
        let compat = capture_after(attrs, r"ItemCompatibility=(/[\w/]+)").unwrap_or("");
        let subcategory = temper_compat_label(compat);

        // Find balanced UpgradeEntries={...} block.
        let Some(ue_idx) = attrs.find("UpgradeEntries=") else {
            continue;
        };
        let Some(brace_off) = attrs[ue_idx..].find('{') else {
            continue;
        };
        let brace_idx = ue_idx + brace_off;
        let mut depth: i32 = 0;
        let mut end_idx: Option<usize> = None;
        for (i, ch) in attrs[brace_idx..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end_idx = Some(brace_idx + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end_idx) = end_idx else { continue };
        let inner = &attrs[brace_idx + 1..end_idx];

        for chunk in split_top_level_braces(inner) {
            let prefix = match capture_after(&chunk, r"LocalizePrefixTag=(\S+)") {
                Some(s) => s.trim().trim_matches('"').to_string(),
                None => continue,
            };
            if !prefix.contains("/SF/Language/Upgrades/Aspect") {
                continue;
            }
            // Chunks with an empty `Upgrades={}` block and a non-empty
            // `SubUpgrades` are references to another aspect entity rather
            // than real definitions — the referenced entities are pure
            // gameplay-script hooks with no display metadata of their own.
            // These wrapper chunks sometimes carry a stale/copy-pasted
            // LocalizePrefixTag left over from whatever chunk they were
            // duplicated from (seen with `OdeReduceDefenseMod` in
            // OdeRangedAspectManifest wrongly claiming
            // `AspectStaggerCritName`/"Cowp"). We still record them in the
            // by_compat/by_manifest lookup maps, but don't let them become a
            // catalog Item when they collide with a chunk that has a real
            // Upgrades block under the same name.
            let is_reference_only = chunk.contains("Upgrades={}")
                && capture_after(&chunk, r"SubUpgrades=\{\s*(/[\w/]+)").is_some();
            let suffix = capture_after(&chunk, r"LocalizeSuffixTag=(\S+)")
                .map(|s| s.trim().trim_matches('"').to_string());
            let tag = capture_after(&chunk, r"\bTag=(\w+)").map(str::to_string);
            let icon = capture_after(&chunk, r"Icon=(/[\w/.]+)").map(str::to_string);
            let faction = capture_after(&chunk, r"\bFaction=(\w+)").map(str::to_string);

            chunks.push(TemperChunk {
                manifest_path: manifest_path.clone(),
                compat: compat.to_string(),
                subcategory,
                prefix,
                suffix,
                tag,
                icon,
                faction,
                is_reference_only,
            });
        }
    }

    // A LocalizePrefixTag normally identifies one real aspect definition.
    // Ambiguity is judged only from chunks with a real Upgrades block —
    // reference-only wrappers (SubUpgrades pointers) don't count, since a
    // wrapper's own LocalizePrefixTag is sometimes a stale copy-paste rather
    // than an intentional second definition (see OdeReduceDefenseMod wrongly
    // claiming AspectStaggerCritName/"Cowp" in OdeRangedAspectManifest).
    let mut real_tags_by_prefix: HashMap<String, HashSet<String>> = HashMap::new();
    for c in &chunks {
        if !c.is_reference_only {
            real_tags_by_prefix
                .entry(c.prefix.clone())
                .or_default()
                .insert(c.tag.clone().unwrap_or_default());
        }
    }

    // Drop reference-only wrappers whose Tag doesn't match any real
    // definition sharing their LocalizePrefixTag — their name was borrowed
    // by mistake, not authored.
    let chunks: Vec<TemperChunk> = chunks
        .into_iter()
        .filter(|c| {
            if !c.is_reference_only {
                return true;
            }
            match real_tags_by_prefix.get(&c.prefix) {
                Some(real_tags) => real_tags.contains(c.tag.as_deref().unwrap_or("")),
                None => true,
            }
        })
        .collect();

    let mut by_key: HashMap<String, Item> = HashMap::new();
    let mut by_compat: HashMap<String, Vec<TemperRef>> = HashMap::new();
    let mut by_manifest: HashMap<String, Vec<TemperRef>> = HashMap::new();

    for c in chunks {
        let TemperChunk {
            manifest_path,
            compat,
            subcategory,
            prefix,
            suffix,
            tag,
            icon,
            faction,
            is_reference_only: _,
        } = c;

        let ambiguous = real_tags_by_prefix
            .get(prefix.as_str())
            .is_some_and(|set| set.len() > 1);

        let tail = prefix.rsplit('/').next().unwrap_or("");
        let base_unique_name = re(r"Name$").replace(tail, "").to_string();
        let unique_name = if ambiguous {
            format!("{base_unique_name}__{}", tag.as_deref().unwrap_or("Unknown"))
        } else {
            base_unique_name
        };
        let display_name = lang
            .get(&prefix)
            .cloned()
            .unwrap_or_else(|| unique_name.clone());

        let dedup_key = if ambiguous {
            format!("{prefix}\u{0}{}", tag.as_deref().unwrap_or(""))
        } else {
            prefix.clone()
        };

        // Build a TemperRef and register it in both lookup maps.
        // The maps are not deduplicated here — each manifest records its
        // own full set of tempers so callers can look up by either
        // manifest path or ItemCompatibility value.
        let temper_ref = TemperRef {
            unique_name: unique_name.clone(),
            name: display_name.clone(),
            lang_key: prefix.clone(),
            lang_desc_key: suffix.clone(),
            icon: icon.clone(),
            faction: faction.clone(),
            tag: tag.clone(),
        };
        if !compat.is_empty() {
            by_compat
                .entry(compat.to_string())
                .or_default()
                .push(temper_ref.clone());
        }
        by_manifest
            .entry(manifest_path.clone())
            .or_default()
            .push(temper_ref);

        // Prefer faction-tagged versions when names collide (Item dedup only).
        if let Some(existing) = by_key.get(&dedup_key) {
            if existing.faction.is_some() || faction.is_none() {
                continue;
            }
        }

        by_key.insert(
            dedup_key,
            Item {
                name: display_name,
                unique_name: unique_name.clone(),
                path: format!("{}#{}", manifest_path, unique_name),
                lang_key: Some(prefix.clone()),
                lang_desc_key: suffix,
                category: "Temper".into(),
                subcategory: subcategory.into(),
                rarity: None,
                icon,
                crafting_recipe: None,
                ingredients: None,
                ingredient_alternatives: None,
                tag,
                faction,
                source_manifest: Some(manifest_path.clone()),
                tradeable: None,
                virtues: None,
                possible_tempers: None,
                can_be_shiny: None,
            },
        );
    }

    (by_key.into_values().collect(), by_compat, by_manifest)
}

// ─── temper / shiny resolvers ─────────────────────────────────────────────

/// Resolve the set of possible tempers for an item.
///
/// The authoritative signal is an `ItemType=` value inside the weapon's
/// `DefaultUpgrades={…}` block that points to an `*AspectManifest` path.
/// Only final catalogue entries classified as weapons should call this resolver;
/// non-weapon upgrade records can contain unrelated aspect paths in shifted or
/// inherited data and must not expose `possible_tempers`.
///
/// A named-attribute fallback (`Aspects=`, `AspectPool=`, `AspectManifest=`,
/// …) is kept for forward-compat if the data ever exposes the link directly.
///
/// Returns `None` when the item has no aspect-manifest binding.
fn resolve_possible_tempers(
    attrs: &str,
    by_manifest: &HashMap<String, Vec<TemperRef>>,
) -> Option<Vec<TemperRef>> {
    // Primary: every ItemType= inside DefaultUpgrades/IntrinsicUpgrades that
    // references an AspectManifest.
    let mut from_upgrades: Vec<TemperRef> = Vec::new();
    for cap in re(r"ItemType=(/[\w/]+AspectManifest)").captures_iter(attrs) {
        if let Some(refs) = by_manifest.get(&cap[1]) {
            from_upgrades.extend(refs.iter().cloned());
        }
    }
    if !from_upgrades.is_empty() {
        return Some(from_upgrades);
    }
    // Fallback: explicit named aspect-pool attribute.
    let manifest_ref = capture_after(attrs, r"Aspects=(/[\w/]+)")
        .or_else(|| capture_after(attrs, r"AspectPool=(/[\w/]+)"))
        .or_else(|| capture_after(attrs, r"AspectManifest=(/[\w/]+)"))
        .or_else(|| capture_after(attrs, r"WeaponAspects=(/[\w/]+)"))
        .or_else(|| capture_after(attrs, r"IntrinsicAspects=(/[\w/]+)"));
    if let Some(mref) = manifest_ref {
        if let Some(refs) = by_manifest.get(mref) {
            if !refs.is_empty() {
                return Some(refs.clone());
            }
        }
    }
    None
}

/// Detect whether an item can have a moonsteel (shiny / gilded) visual variant.
///
/// In current game data, moonsteel variants only apply to weapons that have
/// rollable tempers — i.e. weapons with an `*AspectManifest` reference in
/// their `DefaultUpgrades`.  `ReforgeBlueprint` recipes are the Kintsugi
/// aspect-reroll mechanic (universal to all reforgeable weapons) and are
/// NOT a moonsteel signal.
///
/// A direct-attribute fallback is kept for forward-compat.
fn resolve_can_be_shiny(attrs: &str, has_tempers: bool) -> Option<bool> {
    if has_tempers {
        return Some(true);
    }
    let shiny_attr = capture_after(attrs, r"GildedVariant=(\S+)")
        .or_else(|| capture_after(attrs, r"ShinyVariant=(\S+)"))
        .or_else(|| capture_after(attrs, r"HasShinyVariant=(\w+)"))
        .or_else(|| capture_after(attrs, r"HasMoonsteel=(\w+)"))
        .or_else(|| capture_after(attrs, r"CanBeGilded=(\w+)"))
        .or_else(|| capture_after(attrs, r"\bShiney=(\w+)"))
        .or_else(|| capture_after(attrs, r"\bShiny=(\w+)"));
    if let Some(v) = shiny_attr {
        if v.starts_with('/') {
            return Some(true);
        }
        return Some(matches!(v, "true" | "True" | "1" | "yes"));
    }
    None
}

// ─── build_all_items ──────────────────────────────────────────────────────

fn ingredients_to_value(ings: &[RecipeIngredient]) -> Vec<Value> {
    ings.iter()
        .map(|ing| {
            json!({
                "item": ing.item,
                "item_path": ing.item_path,
                "count": ing.count,
            })
        })
        .collect()
}

fn ingredient_alt_map(ings: &[RecipeIngredient]) -> Option<Map<String, Value>> {
    let mut m = Map::new();
    for ing in ings {
        if let Some(alts) = &ing.alternatives {
            if !alts.is_null() {
                m.insert(ing.item.clone(), alts.clone());
            }
        }
    }
    if m.is_empty() {
        None
    } else {
        Some(m)
    }
}

fn humanise(uname: &str) -> String {
    let s = re(r"([a-z])([A-Z])")
        .replace_all(uname, "$1 $2")
        .to_string();
    re(r"([A-Z]+)([A-Z][a-z])")
        .replace_all(&s, "$1 $2")
        .to_string()
}

pub fn build_all_items(
    pkg_attrs: &PkgAttrs,
    lang: &Languages,
    recipes_list: &[Recipe],
    quiet: bool,
) -> (Value, Value, AvaApiCatalogue, Value) {
    let log = |msg: &str| {
        if !quiet {
            println!("{msg}");
        }
    };

    let mut recipe_by_result: HashMap<&str, &Recipe> = HashMap::new();
    let mut recipe_by_path: HashMap<&str, &Recipe> = HashMap::new();
    for rec in recipes_list {
        recipe_by_path.insert(rec.path.as_str(), rec);
        if let Some(r) = rec.result_item.as_deref() {
            // Prefer build blueprints over Reforge blueprints. When multiple
            // recipes share the same result item, the Reforge (aspect-reroll)
            // recipe must not overwrite the primary crafting recipe.
            let this_is_reforge = rec.path.contains("Reforge");
            let existing_is_reforge = recipe_by_result
                .get(r)
                .map_or(true, |e| e.path.contains("Reforge"));
            if existing_is_reforge || !this_is_reforge {
                recipe_by_result.insert(r, rec);
            }
        }
    }

    // Build temper lookup maps early so the main item loop can link weapons
    // to their possible tempers without a second pass over pkg_attrs.
    log("Extracting Tempers (weapon Aspects) ...");
    let (temper_items, _by_compat, by_manifest) = extract_tempers(pkg_attrs, lang);
    log(&format!("  {} Tempers extracted", temper_items.len()));

    log("\nExtracting items ...");
    let mut all_items: Vec<Item> = Vec::new();
    let mut seen_paths: HashSet<String> = HashSet::new();

    let skip_substrs = [
        "/Fx/",
        "/Sounds/",
        "/Characters/",
        "/Objects/",
        "/Editor/",
        "/Enemies/",
        "/MeleeTrees/",
        "/MeleeAttackSets/",
        "/MeleeAttacks/",
        "/Finishers/",
        "/HitEffects/",
        "/ModConditions/",
        "/Ammo/",
        "/Warriors/",
        "/NeutralPeople/",
        "/GameplayProps/",
        "/LootContainers/",
        "/Objective/",
        "/Gameplay/",
    ];

    let mut sorted_paths: Vec<&String> = pkg_attrs.keys().collect();
    sorted_paths.sort();

    for path in sorted_paths {
        let entry = &pkg_attrs[path];
        let attrs = entry.attributes.as_str();
        let parent = entry.parent.as_str();

        if !is_item_entity(path, parent, attrs) {
            continue;
        }
        if skip_substrs.iter().any(|p| path.contains(p)) && !attrs.contains("LocalizeTag=") {
            continue;
        }
        // Entities flagged ShowInInventory=0 are invisible to players (default-slot
        // proxies, internal wrappers, etc.) and should not appear in the catalogue.
        if attrs.contains("ShowInInventory=0") && !is_player_facing_sparrow_skin(path, attrs) {
            continue;
        }

        let unique_name = path.rsplit('/').next().unwrap_or("").to_string();

        let loc_tag = capture_after(attrs, r"LocalizeTag=(/[\w/]+)").map(str::to_string);
        let loc_desc = capture_after(attrs, r"LocalizeDescTag=(/[\w/]+)").map(str::to_string);
        let rarity = capture_after(attrs, r"Rarity=(\w+)").map(str::to_string);
        let icon = capture_after(attrs, r"Icon=(/[\w/.]+)").map(str::to_string);
        // Trading support: check for common tradeable attribute names.
        // Returns None when the key is absent (i.e. trading not yet flagged in data).
        let tradeable = parse_tradeable(attrs);
        let display_name = match loc_tag.as_deref().and_then(|t| lang.get(t)) {
            Some(s) => s.clone(),
            None => humanise(&unique_name),
        };

        let (category, subcategory) = classify_item(path, parent, attrs);
        if category == "_SKIP" {
            continue;
        }
        let virtues = resolve_item_virtues(category, path, attrs, &pkg_attrs);

        // Recipe cross-reference
        let mut crafting_recipe_ref: Option<String> = None;
        let mut crafting_ingredients: Option<Vec<RecipeIngredient>> = None;

        if let Some(rec) = recipe_by_result.get(path.as_str()) {
            crafting_recipe_ref = Some(rec.path.clone());
            crafting_ingredients = Some(rec.ingredients.clone());
        } else {
            let store_paths = [
                path.replace("/SF/Upgrades/", "/SF/StoreItems/Upgrades/"),
                path.replace("/SF/Weapons/", "/SF/StoreItems/Weapons/"),
                path.replace("/SF/Types/", "/SF/StoreItems/Types/"),
                path.replace("/SF/Souls/", "/SF/StoreItems/Souls/"),
            ];
            for sp in &store_paths {
                if sp != path {
                    if let Some(rec) = recipe_by_result.get(sp.as_str()) {
                        crafting_recipe_ref = Some(rec.path.clone());
                        crafting_ingredients = Some(rec.ingredients.clone());
                        break;
                    }
                }
            }
        }

        if let Some(rec) = recipe_by_path.get(path.as_str()) {
            if crafting_ingredients.is_none() {
                crafting_recipe_ref = Some(path.clone());
                crafting_ingredients = Some(rec.ingredients.clone());
            }
        }

        if crafting_recipe_ref.is_none() {
            if let Some(tn) = capture_after(attrs, r"TypeName=(/[\w/]+)") {
                if let Some(rec) = recipe_by_result.get(tn) {
                    crafting_recipe_ref = Some(rec.path.clone());
                    crafting_ingredients = Some(rec.ingredients.clone());
                }
            }
        }

        let (ingredients_val, alt_map) = match &crafting_ingredients {
            Some(ings) => (Some(ingredients_to_value(ings)), ingredient_alt_map(ings)),
            None => (None, None),
        };

        let possible_tempers = if category == "Weapon" {
            resolve_possible_tempers(attrs, &by_manifest)
        } else {
            None
        };
        let can_be_shiny = resolve_can_be_shiny(attrs, possible_tempers.is_some());

        all_items.push(Item {
            name: display_name,
            unique_name,
            path: path.clone(),
            lang_key: loc_tag,
            lang_desc_key: loc_desc,
            category: category.to_string(),
            subcategory: subcategory.to_string(),
            rarity,
            icon,
            crafting_recipe: crafting_recipe_ref,
            ingredients: ingredients_val,
            ingredient_alternatives: alt_map,
            tag: None,
            faction: None,
            source_manifest: None,
            tradeable,
            virtues,
            possible_tempers,
            can_be_shiny,
        });
        seen_paths.insert(path.clone());
        // For weapons stored at /SF/Weapons/ (without a /StoreItems/ counterpart),
        // also mark the derived StoreItems-equivalent path so the recipe fallback
        // below does not create a duplicate synthetic entry for that path.
        if path.contains("/SF/Weapons/") {
            seen_paths.insert(path.replace("/SF/Weapons/", "/SF/StoreItems/Weapons/"));
        }
    }

    // temper_items were already extracted (and maps built) above the main loop.
    for t in temper_items {
        seen_paths.insert(t.path.clone());
        all_items.push(t);
    }

    log("Cross-referencing recipe result items ...");
    let cat_map: HashMap<&str, (&str, &str)> = [
        ("Armor", ("Armor", "Crafted")),
        ("Weapons", ("Weapon", "Crafted")),
        ("Totems", ("Totem", "Crafted")),
        ("Consumables", ("Consumable", "Crafted")),
        ("Resources", ("Resource", "Crafted")),
        ("Keys", ("Key Item", "Crafted")),
        ("Skins", ("Cosmetic", "Crafted Skin")),
    ]
    .into_iter()
    .collect();

    for rec in recipes_list {
        let Some(result_path) = rec.result_item.as_deref() else {
            continue;
        };
        if seen_paths.contains(result_path) {
            continue;
        }
        if rec.path.contains("Reforge") {
            continue;
        }
        let unique_name = result_path.rsplit('/').next().unwrap_or("").to_string();
        if base_types().contains(unique_name.as_str()) {
            continue;
        }
        // Prefer the result item's own attrs from pkg_attrs over the recipe's
        // (which describes the blueprint, not the crafted item).
        let result_entry = pkg_attrs.get(result_path);
        let result_class = result_entry
            .map(|e| classify_item(result_path, e.parent.as_str(), e.attributes.as_str()));
        if matches!(result_class, Some(("_SKIP", _))) {
            continue;
        }
        let item_lang_key: Option<String> = result_entry
            .and_then(|e| capture_after(&e.attributes, r"LocalizeTag=(/[\w/]+)"))
            .map(str::to_string)
            .or_else(|| rec.localize_tag.clone());
        let item_lang_desc_key: Option<String> = result_entry
            .and_then(|e| capture_after(&e.attributes, r"LocalizeDescTag=(/[\w/]+)"))
            .map(str::to_string);
        let item_icon: Option<String> = result_entry
            .and_then(|e| capture_after(&e.attributes, r"Icon=(/[\w/.]+)"))
            .map(str::to_string)
            .or_else(|| rec.icon.clone());
        let item_tradeable = result_entry.and_then(|e| parse_tradeable(e.attributes.as_str()));
        let display_name = item_lang_key
            .as_deref()
            .and_then(|t| lang.get(t).cloned())
            .unwrap_or_else(|| humanise(&unique_name));

        let (category, subcategory) = result_class
            .or_else(|| cat_map.get(rec.category.as_str()).copied())
            .unwrap_or(("Crafted Item", rec.category.as_str()));
        let item_virtues = result_entry.and_then(|e| {
            resolve_item_virtues(category, result_path, e.attributes.as_str(), &pkg_attrs)
        });

        let ingredients_val = Some(ingredients_to_value(&rec.ingredients));
        let alt_map = ingredient_alt_map(&rec.ingredients);

        all_items.push(Item {
            name: display_name,
            unique_name,
            path: result_path.to_string(),
            lang_key: item_lang_key,
            lang_desc_key: item_lang_desc_key,
            category: category.to_string(),
            subcategory: subcategory.to_string(),
            rarity: rec.rarity.clone(),
            icon: item_icon,
            crafting_recipe: Some(rec.path.clone()),
            ingredients: ingredients_val,
            ingredient_alternatives: alt_map,
            tag: None,
            faction: None,
            source_manifest: None,
            tradeable: item_tradeable,
            virtues: item_virtues,
            possible_tempers: None,
            can_be_shiny: None,
        });
        seen_paths.insert(result_path.to_string());
    }

    // Soulframe-only filter + drop tables & _SKIP.
    let sf_items: Vec<Item> = all_items
        .into_iter()
        .filter(is_soulframe_relevant)
        .filter(|i| i.category != "Drop Table" && i.category != "_SKIP")
        .collect();

    // Dedup by (unique_name, category).
    let mut by_uc: BTreeMap<(String, String), Vec<Item>> = BTreeMap::new();
    for item in sf_items {
        by_uc
            .entry((item.unique_name.clone(), item.category.clone()))
            .or_default()
            .push(item);
    }

    fn merge_into(target: &mut Item, source: &Item) {
        if source.crafting_recipe.is_some() && target.crafting_recipe.is_none() {
            target.crafting_recipe = source.crafting_recipe.clone();
        }
        if source.ingredients.is_some() && target.ingredients.is_none() {
            target.ingredients = source.ingredients.clone();
        }
        if source.lang_key.is_some() && target.lang_key.is_none() {
            target.lang_key = source.lang_key.clone();
        }
        if source.lang_desc_key.is_some() && target.lang_desc_key.is_none() {
            target.lang_desc_key = source.lang_desc_key.clone();
        }
        if source.icon.is_some() && target.icon.is_none() {
            target.icon = source.icon.clone();
        }
        if source.tradeable.is_some() && target.tradeable.is_none() {
            target.tradeable = source.tradeable;
        }
        if source.virtues.is_some() && target.virtues.is_none() {
            target.virtues = source.virtues.clone();
        }
        if source.possible_tempers.is_some() && target.possible_tempers.is_none() {
            target.possible_tempers = source.possible_tempers.clone();
        }
        if source.can_be_shiny.is_some() && target.can_be_shiny.is_none() {
            target.can_be_shiny = source.can_be_shiny;
        }
    }

    fn score(e: &Item) -> (bool, bool, bool, usize) {
        (
            e.ingredients.is_some(),
            e.crafting_recipe.is_some(),
            e.icon.is_some(),
            e.path.len(),
        )
    }

    let mut final_items: Vec<Item> = Vec::new();
    for (_key, entries) in by_uc {
        if entries.len() == 1 {
            final_items.extend(entries);
            continue;
        }

        let mut named: BTreeMap<String, Vec<Item>> = BTreeMap::new();
        let mut unnamed: Vec<Item> = Vec::new();
        for e in entries {
            match e.lang_key.clone() {
                Some(lk) => named.entry(lk).or_default().push(e),
                None => unnamed.push(e),
            }
        }

        let mut named_bests: Vec<Item> = Vec::new();
        for (_lk, group) in named {
            let mut iter = group.into_iter();
            let mut best = iter.next().unwrap();
            let mut others: Vec<Item> = Vec::new();
            for cand in iter {
                if score(&cand) > score(&best) {
                    others.push(std::mem::replace(&mut best, cand));
                } else {
                    others.push(cand);
                }
            }
            for o in &others {
                merge_into(&mut best, o);
            }
            named_bests.push(best);
        }

        if !named_bests.is_empty() {
            if let Some(first) = named_bests.first_mut() {
                for u in &unnamed {
                    merge_into(first, u);
                }
            }
            final_items.extend(named_bests);
        } else {
            let mut iter = unnamed.into_iter();
            let mut best = iter.next().unwrap();
            let mut others: Vec<Item> = Vec::new();
            for cand in iter {
                if score(&cand) > score(&best) {
                    others.push(std::mem::replace(&mut best, cand));
                } else {
                    others.push(cand);
                }
            }
            for o in &others {
                merge_into(&mut best, o);
            }
            final_items.push(best);
        }
    }

    final_items.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then(a.subcategory.cmp(&b.subcategory))
            .then(a.name.cmp(&b.name))
    });

    // Category counts.
    let mut cat_counts: BTreeMap<String, usize> = BTreeMap::new();
    for item in &final_items {
        *cat_counts.entry(item.category.clone()).or_insert(0) += 1;
    }

    // Ingredient index.
    let mut ingredient_index: BTreeMap<String, Value> = BTreeMap::new();
    for item in &final_items {
        let Some(ings) = item.ingredients.as_ref() else {
            continue;
        };
        for ing in ings {
            let ing_name = ing
                .get("item")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let ing_path = ing.get("item_path").cloned().unwrap_or(Value::Null);
            let entry = ingredient_index.entry(ing_name.clone()).or_insert_with(|| {
                json!({
                    "item_path": ing_path,
                    "used_by": Vec::<Value>::new(),
                })
            });
            if let Some(arr) = entry.get_mut("used_by").and_then(Value::as_array_mut) {
                arr.push(json!({
                    "item": item.name,
                    "item_path": item.path,
                    "count": ing.get("count").cloned().unwrap_or(Value::Null),
                }));
            }
        }
    }

    let generated_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    log(&format!(
        "\n  all_items catalogue: {} items across {} categories",
        final_items.len(),
        cat_counts.len()
    ));
    let mut ordered_counts: Vec<(&String, &usize)> = cat_counts.iter().collect();
    ordered_counts.sort_by(|a, b| b.1.cmp(a.1));
    for (cat, n) in &ordered_counts {
        log(&format!("    {:>5}  {}", n, cat));
    }

    // Build secondary catalogues before consuming final_items into the full catalogue.
    let market = build_market_items(&final_items, lang, &generated_at);
    let ava_api = build_ava_api_items(&final_items, &cat_counts, &generated_at);
    let weapons = build_weapons_catalogue(&final_items, pkg_attrs, lang, &generated_at);

    let catalogue = json!({
        "metadata": {
            "title": "Soulframe - All Items",
            "description": "Every item in Soulframe: weapons, armor, totems, motes, runes, blueprints, resources, materials, currencies, cosmetics, consumables, and more. Each entry includes the internal path, language key for localisation, crafting recipe reference, and required ingredients.",
            "generated_at": generated_at,
            "source": "Packages.bin (H.Misc cache) + Languages.json + crafting_recipes.json",
            "total_items": final_items.len(),
            "categories": cat_counts,
            "total_unique_ingredients": ingredient_index.len(),
        },
        "items": final_items,
        "ingredient_index": ingredient_index,
    });

    (catalogue, market, ava_api, weapons)
}

// ─── build_ava_api_items ──────────────────────────────────────────────────

/// Build the legacy all-items-style catalogue consumed by the AVA API.
fn build_ava_api_items(
    items: &[Item],
    cat_counts: &BTreeMap<String, usize>,
    generated_at: &str,
) -> AvaApiCatalogue {
    let ingredient_index = build_ava_api_ingredient_index(items);
    let ava_items: Vec<AvaApiItem> = items.iter().map(AvaApiItem::from).collect();
    AvaApiCatalogue {
        metadata: AvaApiMetadata {
            title: "Soulframe - All Items",
            description: "Every item in Soulframe: weapons, armor, totems, motes, runes, blueprints, resources, materials, currencies, cosmetics, consumables, and more. Each entry includes the internal path, language key for localisation, crafting recipe reference, and required ingredients.",
            generated_at: generated_at.to_string(),
            source: "Packages.bin (H.Misc cache) + Languages.json + crafting_recipes.json",
            total_items: items.len(),
            categories: cat_counts.clone(),
            total_unique_ingredients: ingredient_index.len(),
        },
        items: ava_items,
        ingredient_index,
    }
}

fn build_ava_api_ingredient_index(items: &[Item]) -> BTreeMap<String, AvaApiIngredientIndexEntry> {
    let mut index = BTreeMap::new();
    for item in items {
        let Some(ingredients) = item.ingredients.as_ref() else {
            continue;
        };
        for ingredient in ingredients.iter().map(ava_api_ingredient) {
            let entry = index.entry(ingredient.item.clone()).or_insert_with(|| {
                AvaApiIngredientIndexEntry {
                    item_path: ingredient.item_path.clone(),
                    used_by: Vec::new(),
                }
            });
            entry.used_by.push(AvaApiIngredientUse {
                item: item.name.clone(),
                item_path: item.path.clone(),
                count: ingredient.count,
            });
        }
    }
    index
}

// ─── build_market_items ───────────────────────────────────────────────────

/// Build a compact market-focused catalogue from the finalised item list.
///
/// Each entry contains:
/// - Resolved display name and description (in the language that was supplied
///   to `build_all_items`, typically English).
/// - `lang_name_key` / `lang_desc_key` — pointers into the per-locale
///   language files so consumers can swap language without re-running the
///   pipeline.
/// - `icon` — the raw in-game asset path (e.g.
///   `/SF/Interface/Graphics/StoreItems/Cuirass/BromiusCuirass.png`).
/// - `image_path` — the same path with the leading `/` stripped, suitable
///   for joining onto an HTTP images root (the soulkit-cli `images` stage
///   extracts assets to `output/images/<image_path>`).
/// Only items explicitly flagged `tradeable: true` in the game data are
/// included.  Items with a `null` tradeable flag (flag absent from game data)
/// are omitted.
fn build_market_items(items: &[Item], lang: &Languages, generated_at: &str) -> Value {
    let market_entries: Vec<Value> = items
        .iter()
        .filter(|item| item.tradeable == Some(true))
        .map(|item| {
            let description = item
                .lang_desc_key
                .as_deref()
                .and_then(|k| lang.get(k))
                .cloned()
                .unwrap_or_default();
            let image_path = item
                .icon
                .as_deref()
                .map(|p| p.trim_start_matches('/').to_string());
            json!({
                "unique_name": item.unique_name,
                "path": item.path,
                "name": item.name,
                "description": description,
                "lang_name_key": item.lang_key,
                "lang_desc_key": item.lang_desc_key,
                "category": item.category,
                "subcategory": item.subcategory,
                "rarity": item.rarity,
                "tradeable": item.tradeable,
                "virtues": item.virtues,
                "icon": item.icon,
                "image_path": image_path,
                "possible_tempers": item.possible_tempers,
                "can_be_shiny": item.can_be_shiny,
            })
        })
        .collect();

    json!({
        "metadata": {
            "title": "Soulframe - Market Items",
            "description": "Compact catalogue of tradeable items only (tradeable=true in game data). Includes resolved names, descriptions, and lang-key pointers for localisation consumers.",
            "generated_at": generated_at,
            "source": "Derived from all_items pipeline",
            "total_items": market_entries.len(),
        },
        "items": market_entries,
    })
}

// ─── build_weapons_catalogue ──────────────────────────────────────────────────

/// Parse a simple `KEY={\n  AT_X=N\n}` block from raw attribute text.
///
/// Only works for blocks whose content is plain `KEY=integer` lines with no
/// nested braces (i.e. `Requirements`, `Attunements`, `AttunementsCap`).
fn temper_ref_to_camel(t: &TemperRef) -> Value {
    let mut m = Map::new();
    m.insert(
        "uniqueName".to_string(),
        Value::String(t.unique_name.clone()),
    );
    m.insert("name".to_string(), Value::String(t.name.clone()));
    m.insert("langKey".to_string(), Value::String(t.lang_key.clone()));
    if let Some(k) = &t.lang_desc_key {
        m.insert("langDescKey".to_string(), Value::String(k.clone()));
    }
    if let Some(i) = &t.icon {
        m.insert("icon".to_string(), Value::String(i.clone()));
    }
    if let Some(f) = &t.faction {
        m.insert("faction".to_string(), Value::String(f.clone()));
    }
    if let Some(tag) = &t.tag {
        m.insert("tag".to_string(), Value::String(tag.clone()));
    }
    Value::Object(m)
}

fn parse_virtue_block(attrs: &str, pattern: &'static str) -> Option<BTreeMap<String, i32>> {
    let content = re(pattern).captures(attrs)?.get(1)?.as_str();
    let mut map = BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if let Ok(n) = v.trim().parse::<i32>() {
                map.insert(k.to_string(), n);
            }
        }
    }
    (!map.is_empty()).then_some(map)
}

struct TypeStats {
    damage_type: Option<String>,
    damage: Option<f64>,
    stability_damage: Option<f64>,
    critical_hit_chance: Option<f64>,
    critical_hit_damage_multiplier: Option<f64>,
    fire_rate: Option<f64>,
    charge_damage_multiplier: Option<F64Range>,
    charge_stability_damage_multi: Option<F64Range>,
    perfect_charge_attack_range: Option<F64Range>,
    charge_armour_bypass_pct: Option<F64Range>,
    projectile_amount: Option<f64>,
}

fn extract_type_stats(type_attrs: &str) -> TypeStats {
    let impact = re(r"(?s)MeleeImpactBehavior=\{.*?AttackData=\{.*?Type=(\w+).*?Amount=(\d+(?:\.\d+)?).*?StabilityDamage=(\d+(?:\.\d+)?)")
        .captures(type_attrs);
    let damage_type = impact
        .as_ref()
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
    let damage = impact
        .as_ref()
        .and_then(|c| c.get(2).and_then(|m| m.as_str().parse::<f64>().ok()));
    let stability_damage = impact
        .as_ref()
        .and_then(|c| c.get(3).and_then(|m| m.as_str().parse::<f64>().ok()));

    let critical_hit_chance = capture_after(type_attrs, r"criticalHitChance=([\d.]+)")
        .and_then(|v| v.parse::<f64>().ok());
    let critical_hit_damage_multiplier =
        capture_after(type_attrs, r"criticalHitDamageMultiplier=([\d.]+)")
            .and_then(|v| v.parse::<f64>().ok());
    let fire_rate =
        capture_after(type_attrs, r"fireRate=([\d.]+)").and_then(|v| v.parse::<f64>().ok());

    let parse_f64_range = |caps: Option<regex::Captures<'_>>| -> Option<F64Range> {
        caps.and_then(|c| {
            let min = c.get(1)?.as_str().parse::<f64>().ok()?;
            let max = c.get(2)?.as_str().parse::<f64>().ok()?;
            Some(F64Range { min, max })
        })
    };

    let charge_damage_multiplier =
        parse_f64_range(re(r"ChargeDamageMultiplier=\{([\d.]+),([\d.]+)\}").captures(type_attrs));
    let charge_stability_damage_multi = parse_f64_range(
        re(r"ChargeStabilityDamageMulti=\{([\d.]+),([\d.]+)\}").captures(type_attrs),
    );
    let perfect_charge_attack_range =
        parse_f64_range(re(r"PerfectChargeAttackRange=\{([\d.]+),([\d.]+)\}").captures(type_attrs));
    let charge_armour_bypass_pct =
        parse_f64_range(re(r"ChargeArmourBypassPct=\{([\d.]+),([\d.]+)\}").captures(type_attrs));

    // Quick-fire / ranged projectile damage: the MeleeImpactBehavior block just before
    // the SFCasterRangedStateBehavior (staves) or SFBowRangedStateBehavior (bows) entry.
    let ranged_state_marker = [
        "state:SFCasterRangedStateBehavior=",
        "state:SFBowRangedStateBehavior=",
    ]
    .iter()
    .filter_map(|&marker| type_attrs.find(marker).map(|pos| (pos, marker)))
    .min_by_key(|&(pos, _)| pos)
    .map(|(pos, _)| pos);

    let projectile_amount = ranged_state_marker.and_then(|marker_pos| {
        let pre = &type_attrs[..marker_pos];
        let impact_pos = pre.rfind("MeleeImpactBehavior=")?;
        let impact_section = &pre[impact_pos..];
        re(r"Amount=(\d+(?:\.\d+)?)")
            .captures(impact_section)
            .and_then(|c| c.get(1)?.as_str().parse::<f64>().ok())
    });

    TypeStats {
        damage_type,
        damage,
        stability_damage,
        critical_hit_chance,
        critical_hit_damage_multiplier,
        fire_rate,
        charge_damage_multiplier,
        charge_stability_damage_multi,
        perfect_charge_attack_range,
        charge_armour_bypass_pct,
        projectile_amount,
    }
}

/// Build the weapons-focused catalogue from the already-finalised item list.
pub fn build_weapons_catalogue(
    items: &[Item],
    pkg_attrs: &PkgAttrs,
    lang: &Languages,
    generated_at: &str,
) -> Value {
    let weapons: Vec<WeaponItem> = items
        .iter()
        .filter(|i| i.category == "Weapon")
        .map(|item| {
            let store_attrs = pkg_attrs
                .get(&item.path)
                .map(|e| e.attributes.as_str())
                .unwrap_or("");

            // For crafted weapons whose store item isn't in pkg_attrs, derive the
            // TypeName path from the item path (/SF/StoreItems/... → /SF/...).
            let derived_type_name = if store_attrs.is_empty() && item.path.contains("/StoreItems/")
            {
                item.path.replace("/StoreItems/", "/")
            } else {
                String::new()
            };

            let type_attrs_owned: Option<String> =
                capture_after(store_attrs, r"TypeName=(/[\w/]+)")
                    .and_then(|tn| pkg_attrs.get(tn))
                    .map(|e| e.attributes.clone())
                    .or_else(|| {
                        if !derived_type_name.is_empty() {
                            pkg_attrs
                                .get(derived_type_name.as_str())
                                .map(|e| e.attributes.clone())
                        } else {
                            None
                        }
                    });
            let type_attrs: &str = type_attrs_owned.as_deref().unwrap_or("");

            let ts = extract_type_stats(type_attrs);

            let grip_type = capture_after(store_attrs, r"GripType=(\w+)").map(str::to_string);
            let melee_style = capture_after(store_attrs, r"MeleeStyle=(\w+)").map(str::to_string);
            let product_category =
                capture_after(store_attrs, r"ProductCategory=(\w+)").map(str::to_string);

            // ItemType may live in the store item or (for crafted weapons) the TypeName entity.
            let source_manifest =
                capture_after(store_attrs, r"ItemType=(/SF/Types/Items/Aspects/[\w/]+)")
                    .map(str::to_string)
                    .or_else(|| {
                        capture_after(type_attrs, r"ItemType=(/SF/Types/Items/Aspects/[\w/]+)")
                            .map(str::to_string)
                    });

            let requirements = parse_virtue_block(store_attrs, r"(?s)Requirements=\{([^}]+)\}")
                .or_else(|| parse_virtue_block(type_attrs, r"(?s)Requirements=\{([^}]+)\}"));
            let attunements = parse_virtue_block(store_attrs, r"(?s)Attunements=\{([^}]+)\}")
                .or_else(|| parse_virtue_block(type_attrs, r"(?s)Attunements=\{([^}]+)\}"));
            let attunements_cap =
                parse_virtue_block(store_attrs, r"(?s)AttunementsCap=\{([^}]+)\}");

            let description = item
                .lang_desc_key
                .as_deref()
                .and_then(|k| lang.get(k))
                .cloned();

            let possible_tempers = item
                .possible_tempers
                .as_ref()
                .map(|v| v.iter().map(temper_ref_to_camel).collect::<Vec<Value>>());

            WeaponItem {
                name: item.name.clone(),
                unique_name: item.unique_name.clone(),
                path: item.path.clone(),
                lang_key: item.lang_key.clone(),
                lang_desc_key: item.lang_desc_key.clone(),
                description,
                category: item.category.clone(),
                subcategory: item.subcategory.clone(),
                rarity: item.rarity.clone(),
                icon: item.icon.clone(),
                crafting_recipe: item.crafting_recipe.clone(),
                ingredients: item.ingredients.clone(),
                ingredient_alternatives: item.ingredient_alternatives.clone(),
                tradeable: item.tradeable,
                can_be_shiny: item.can_be_shiny,
                virtues: item.virtues.clone(),
                possible_tempers,
                source_manifest,
                grip_type,
                melee_style,
                product_category,
                damage: ts.damage,
                damage_type: ts.damage_type,
                stability_damage: ts.stability_damage,
                fire_rate: ts.fire_rate,
                critical_hit_chance: ts.critical_hit_chance,
                critical_hit_damage_multiplier: ts.critical_hit_damage_multiplier,
                charge_damage_multiplier: ts.charge_damage_multiplier,
                charge_stability_damage_multi: ts.charge_stability_damage_multi,
                perfect_charge_attack_range: ts.perfect_charge_attack_range,
                charge_armour_bypass_pct: ts.charge_armour_bypass_pct,
                projectile_amount: ts.projectile_amount,
                requirements,
                attunements,
                attunements_cap,
            }
        })
        .collect();

    let total = weapons.len();
    json!({
        "metadata": {
            "title": "Soulframe - Weapons",
            "description": "All player-accessible weapons with combat stats, grip types, virtue requirements, attunements, and possible tempers. Stats sourced from weapon entity definitions in Packages.bin.",
            "generated_at": generated_at,
            "source": "Packages.bin (H.Misc cache) + Languages.json",
            "total_weapons": total,
        },
        "weapons": weapons,
    })
}

pub fn run(
    pkg_attrs_path: &Path,
    languages_path: &Path,
    recipes_path: &Path,
    output: &Path,
) -> Result<()> {
    println!("Loading {} ...", pkg_attrs_path.display());
    let pkg_attrs: PkgAttrs = serde_json::from_str(
        &fs::read_to_string(pkg_attrs_path)
            .with_context(|| format!("Reading {:?}", pkg_attrs_path))?,
    )
    .context("Parsing packages_bin_attrs.json")?;
    println!("  {} entities", pkg_attrs.len());

    println!("Loading {} ...", languages_path.display());
    let lang = load_languages(languages_path)?;
    println!("  {} language keys", lang.len());

    println!("Loading {} ...", recipes_path.display());
    let recipes_list: Vec<Recipe> = serde_json::from_str(
        &fs::read_to_string(recipes_path).with_context(|| format!("Reading {:?}", recipes_path))?,
    )
    .context("Parsing crafting_recipes.json")?;
    println!("  {} recipes", recipes_list.len());

    let (catalogue, market, ava_api, weapons) =
        build_all_items(&pkg_attrs, &lang, &recipes_list, false);

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(output, serde_json::to_string_pretty(&catalogue)?)?;

    let market_output = output.with_file_name("market_items.json");
    fs::write(&market_output, serde_json::to_string_pretty(&market)?)?;

    let ava_api_output = output.with_file_name("ava_api_items.json");
    fs::write(&ava_api_output, serde_json::to_string_pretty(&ava_api)?)?;

    let weapons_output = output.with_file_name("weapons.json");
    fs::write(&weapons_output, serde_json::to_string_pretty(&weapons)?)?;

    let total = catalogue["metadata"]["total_items"].as_u64().unwrap_or(0);
    let market_total = market["metadata"]["total_items"].as_u64().unwrap_or(0);
    let ava_api_total = ava_api.metadata.total_items;
    let weapons_total = weapons["metadata"]["total_weapons"].as_u64().unwrap_or(0);
    println!("\n{}", "=".repeat(60));
    println!("  all_items.json:     {} items", total);
    println!("  market_items.json:  {} items", market_total);
    println!("  ava_api_items.json: {} items", ava_api_total);
    println!("  weapons.json:       {} weapons", weapons_total);
    println!("  Output: {}", output.display());
    println!("{}", "=".repeat(60));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_armor_helmet_skin() {
        let (c, s) = classify_item(
            "/SF/Upgrades/Skins/Helmets/AvarotMaskHelmet",
            "/SF/Types/Game/SFSuitCustomization",
            "",
        );
        assert_eq!((c, s), ("Armor", "Helmet"));
    }

    #[test]
    fn localized_sparrow_skin_bypasses_hidden_inventory_filter() {
        let attrs = "LocalizeTag=/SF/Language/Cosmetics/GaleneSparrowBodyName\n\
                     Icon=/SF/Interface/Graphics/StoreItems/Guides/GaleneSparrow.png\n\
                     ShowInInventory=0";
        assert!(is_player_facing_sparrow_skin(
            "/SF/Upgrades/Skins/Sparrow/GaleneSparrowBody",
            attrs,
        ));
        assert!(!is_player_facing_sparrow_skin(
            "/SF/Upgrades/Skins/Sparrow/GaleneSparrowBodySkin",
            "TpMeshOverride=/SF/Characters/WildLife/Birds/Galene.fbx",
        ));
        assert!(!is_player_facing_sparrow_skin(
            "/SF/Upgrades/Skins/Sparrow/AnimTrees/GaleneAnimTreeController",
            attrs,
        ));
    }

    #[test]
    fn classify_dendria_weapon_word() {
        let (c, s) = classify_item("/SF/Upgrades/Dendria/Foo/WeaponBar", "", "");
        assert_eq!((c, s), ("Dendria Word", "Weapon Word"));
    }

    #[test]
    fn classify_base_type_skipped() {
        let (c, _) = classify_item("/SF/Types/Game/BaseHelmet", "", "");
        assert_eq!(c, "_SKIP");
    }

    #[test]
    fn classify_recipe_armor_blueprint() {
        let (c, s) = classify_item(
            "/SF/StoreItems/Types/Recipes/Armor/AvarotMaskHelmBlueprint",
            "/SF/Types/Game/StoreItemSpecializations/RecipeStoreItem",
            "",
        );
        assert_eq!((c, s), ("Blueprint", "Armor"));
    }

    #[test]
    fn humanise_camelcase() {
        assert_eq!(humanise("AvarotMaskHelmet"), "Avarot Mask Helmet");
        assert_eq!(humanise("BCBlueprint"), "BC Blueprint");
    }

    #[test]
    fn split_braces_handles_nesting() {
        let v = split_top_level_braces("{a={b={c}}} {d}");
        assert_eq!(v, vec!["a={b={c}}", "d"]);
    }

    #[test]
    fn classify_mote_language_totem_paths_as_motes() {
        let raw = classify_item(
            "/SF/Upgrades/Totems/AoeRip",
            "/Lotus/Types/Game/LotusArtifactUpgrades/BaseArtifactUpgrade",
            "LocalizeTag=/SF/Language/Motes/AoeRipName",
        );
        let store = classify_item(
            "/SF/StoreItems/Upgrades/Totems/AoeRip",
            "/Lotus/Types/Game/StoreItems/AutoStoreItem",
            "LocalizeDescTag=/SF/Language/Motes/AoeRipDesc\nTradeable=1",
        );

        assert_eq!(raw, ("Mote", "General"));
        assert_eq!(store, raw);
    }

    #[test]
    fn classify_boon_item_bundles_as_bundles() {
        let raw = classify_item(
            "/SF/Upgrades/Boons/Zenith/RareCraftingResourcesBoonItemBundle",
            "/Lotus/Types/Game/LotusArtifactUpgrades/BaseArtifactUpgrade",
            "LocalizeTag=/SF/Language/Upgrades/RareCraftingResourcesBoonItemBundle",
        );
        let store = classify_item(
            "/SF/StoreItems/Upgrades/Boons/Zenith/RareCraftingResourcesBoonItemBundle",
            "/Lotus/Types/Game/StoreItems/AutoStoreItem",
            "LocalizeTag=/SF/Language/Upgrades/RareCraftingResourcesBoonItemBundle\nTradeable=1",
        );

        assert_eq!(raw, ("Bundle", "Boon Bundle"));
        assert_eq!(store, raw);
    }

    #[test]
    fn empty_localize_tag_does_not_make_item_entity() {
        assert!(!is_item_entity(
            "/SF/Upgrades/Skins/Head/BaseFemaleHead",
            "/SF/Upgrades/Skins/Head/BaseHead",
            "LocalizeTag=\"\"\nIcon=\"\"",
        ));
    }

    #[test]
    fn leaked_engine_and_template_records_are_skipped() {
        for (path, parent, attrs) in [
            (
                "/SF/Types/GameRules/SFSandBoxGameRules",
                "/SF/Types/GameRules/",
                "LocalizeTag=/Lotus/Language/Menu/Loadout_Beast",
            ),
            (
                "/SF/Souls/Default/InnateUpgrade",
                "/Lotus/Types/Game/LotusArtifactUpgrades/BaseArtifactUpgrade",
                "LocalizeTag=/Lotus/Language/Mods/WeaponMeleeExposedWeaknessMod",
            ),
            (
                "/SF/Types/Player/HolsterCustomizations/SwordandShield",
                "/Lotus/Types/Game/StoreItems/HolsterCustomization",
                "LocalizeTag=/Lotus/Language/HolsterCustomizations/SwordHipsL",
            ),
            (
                "/SF/Types/Items/Fish/FishItem",
                "/SF/Types/Items/Fish/",
                "LocalizeTag=/Lotus/Language/Items/BothCommonFishA",
            ),
            (
                "/SF/StoreItems/Types/Game/SFSkillTree",
                "/SF/Types/Game/StoreItemSpecializations/SFSkillTreeStoreItem",
                "LocalizeTag=/Lotus/Language/Items/EmptyArtifact",
            ),
            (
                "/SF/Types/Challenges/Complications/SFChallengeComplication",
                "/SF/Types/Challenges/Complications/",
                "LocalizeTag=/SF/Language/Skills/SwordSkill",
            ),
        ] {
            assert_eq!(classify_item(path, parent, attrs).0, "_SKIP");
        }
    }

    #[test]
    fn quest_asset_raw_and_store_paths_share_category() {
        let raw = classify_item(
            "/SF/Types/Quests/AncestorProgression/OrlickProgression/QuestAssets/Resources/VerminaInkResource",
            "/SF/Types/PickUps/Resources/ResourceItem",
            "LocalizeTag=/SF/Language/Materials/VerminaInkName",
        );
        let store = classify_item(
            "/SF/StoreItems/Types/Quests/AncestorProgression/OrlickProgression/QuestAssets/Resources/VerminaInkResource",
            "/Lotus/Types/Game/StoreItems/AutoStoreItem",
            "LocalizeTag=/SF/Language/Materials/VerminaInkName\nTradeable=1",
        );

        assert_eq!(raw, ("Quest Item", "Quest"));
        assert_eq!(store, raw);
    }

    #[test]
    fn quest_definition_raw_and_store_paths_share_category() {
        let raw = classify_item(
            "/SF/Types/Quests/SideQuests/ABirdieFlown",
            "/SF/Types/Quests/",
            "LocalizeTag=/SF/Language/Quests/ABirdieFlownQuestTitle",
        );
        let store = classify_item(
            "/SF/StoreItems/Types/Quests/SideQuests/ABirdieFlown",
            "/Lotus/Types/Game/StoreItems/AutoStoreItem",
            "LocalizeTag=/SF/Language/Quests/ABirdieFlownQuestTitle",
        );

        assert_eq!(raw, ("Quest", "Quest"));
        assert_eq!(store, raw);
    }

    #[test]
    fn ava_api_catalogue_uses_legacy_item_shape() {
        let items = vec![Item {
            name: "Test Item".to_string(),
            unique_name: "TestItem".to_string(),
            path: "/SF/StoreItems/TestItem".to_string(),
            lang_key: Some("/SF/Language/TestItemName".to_string()),
            lang_desc_key: None,
            category: "Resource".to_string(),
            subcategory: "Other".to_string(),
            rarity: Some("COMMON".to_string()),
            icon: Some("/SF/Interface/TestItem.png".to_string()),
            crafting_recipe: Some("/SF/Types/Recipes/TestItemBlueprint".to_string()),
            ingredients: Some(vec![json!({
                "item": "Iron",
                "item_path": "/SF/Types/PickUps/Resources/Iron",
                "count": 1,
            })]),
            ingredient_alternatives: None,
            tag: None,
            faction: None,
            source_manifest: None,
            tradeable: Some(true),
            virtues: Some(vec!["courage".to_string()]),
            possible_tempers: Some(vec![TemperRef {
                unique_name: "AspectTest".to_string(),
                name: "Aspect Test".to_string(),
                lang_key: "/SF/Language/Upgrades/AspectTestName".to_string(),
                lang_desc_key: None,
                icon: None,
                faction: None,
                tag: None,
            }]),
            can_be_shiny: Some(true),
        }];
        let mut cat_counts = BTreeMap::new();
        cat_counts.insert("Resource".to_string(), 1);

        let catalogue = build_ava_api_items(&items, &cat_counts, "2026-06-13T00:00:00Z");
        let rendered = serde_json::to_string_pretty(&catalogue).unwrap();
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        let item = &parsed["items"][0];

        assert_ordered(
            &rendered,
            &[
                "\n  \"metadata\":",
                "\n  \"items\":",
                "\n  \"ingredient_index\":",
            ],
        );
        assert_ordered(
            &rendered,
            &[
                "\n    \"title\":",
                "\n    \"description\":",
                "\n    \"generated_at\":",
                "\n    \"source\":",
                "\n    \"total_items\":",
                "\n    \"categories\":",
                "\n    \"total_unique_ingredients\":",
            ],
        );
        assert_ordered(
            &rendered,
            &[
                "\n      \"name\":",
                "\n      \"unique_name\":",
                "\n      \"path\":",
                "\n      \"lang_key\":",
                "\n      \"lang_desc_key\":",
                "\n      \"category\":",
                "\n      \"subcategory\":",
                "\n      \"rarity\":",
                "\n      \"icon\":",
                "\n      \"crafting_recipe\":",
                "\n      \"ingredients\":",
            ],
        );
        assert_ordered(
            &rendered,
            &[
                "\n          \"item\":",
                "\n          \"item_path\":",
                "\n          \"count\":",
            ],
        );
        let index = &rendered[rendered.find("\n  \"ingredient_index\":").unwrap()..];
        assert_ordered(
            index,
            &[
                "\n      \"item_path\":",
                "\n      \"used_by\":",
                "\n          \"item\":",
                "\n          \"item_path\":",
                "\n          \"count\":",
            ],
        );

        assert_eq!(catalogue.metadata.total_items, 1);
        assert_eq!(catalogue.ingredient_index["Iron"].used_by.len(), 1);
        assert_eq!(
            parsed["ingredient_index"]["Iron"]["used_by"][0]["item"],
            "Test Item"
        );
        assert_eq!(item["name"], "Test Item");
        assert_eq!(item["lang_desc_key"], Value::Null);
        assert_eq!(item["possible_tempers"][0]["unique_name"], "AspectTest");
        assert!(item.get("tradeable").is_none());
        assert!(item.get("virtues").is_none());
        assert!(item.get("can_be_shiny").is_none());
    }

    fn assert_ordered(haystack: &str, needles: &[&str]) {
        let mut offset = 0;
        for needle in needles {
            let pos = haystack[offset..]
                .find(needle)
                .unwrap_or_else(|| panic!("missing ordered fragment {needle:?}"));
            offset += pos + needle.len();
        }
    }
}
