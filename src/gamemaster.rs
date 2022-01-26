//! This is a porting of https://github.com/RealDeviceMap/RealDeviceMap/blob/master/Sources/RealDeviceMapLib/Misc/PVPStatsManager.swift

use std::{sync::Arc, time::{Duration, SystemTime}, collections::{BTreeMap, HashMap}, ops::Deref, marker::PhantomData};

use arc_swap::ArcSwap;

use once_cell::sync::Lazy;

use serde::{Deserialize, Deserializer};

use tokio::time::interval;

use pogo_gamemaster_entities::{TemplateWrapper, EvolutionBranch, PokemonSettings};

use tracing::{error, debug};

use crate::{PvpRanking, Gender};

type Pvp = Option<Vec<PvpRanking>>;

static CP_MULTIPLIERS: [f64; 109] = [
    0.09399999678134918,
    0.13513743132352830,
    0.16639786958694458,
    0.19265091419219970,
    0.21573247015476227,
    0.23657265305519104,
    0.25572004914283750,
    0.27353037893772125,
    0.29024988412857056,
    0.30605737864971160,
    0.32108759880065920,
    0.33544503152370453,
    0.34921267628669740,
    0.36245773732662200,
    0.37523558735847473,
    0.38759241108516856,
    0.39956727623939514,
    0.41119354951725060,
    0.4225000143051148,
    0.4329264134104144,
    0.4431075453758240,
    0.4530599538719858,
    0.4627983868122100,
    0.4723360780626535,
    0.4816849529743195,
    0.4908558102324605,
    0.4998584389686584,
    0.5087017565965652,
    0.5173939466476440,
    0.5259425118565559,
    0.5343543291091919,
    0.5426357612013817,
    0.5507926940917969,
    0.5588305993005633,
    0.5667545199394226,
    0.5745691470801830,
    0.5822789072990417,
    0.5898879119195044,
    0.5974000096321106,
    0.6048236563801765,
    0.6121572852134705,
    0.6194041110575199,
    0.6265671253204346,
    0.6336491815745830,
    0.6406529545783997,
    0.6475809663534164,
    0.6544356346130370,
    0.6612192690372467,
    0.6679340004920960,
    0.6745819002389908,
    0.6811649203300476,
    0.6876849085092545,
    0.6941436529159546,
    0.7005428969860077,
    0.7068842053413391,
    0.7131690979003906,
    0.7193990945816040,
    0.7255756109952927,
    0.7317000031471252,
    0.7347410172224045,
    0.7377694845199585,
    0.7407855764031410,
    0.7437894344329834,
    0.7467812150716782,
    0.7497610449790955,
    0.7527291029691696,
    0.7556855082511902,
    0.7586303651332855,
    0.7615638375282288,
    0.7644860669970512,
    0.7673971652984619,
    0.7702972739934921,
    0.7731865048408508,
    0.7760649472475052,
    0.7789327502250671,
    0.78179006,
    0.78463697,
    0.78747358,
    0.790300011634827,
    0.792803950958808,
    0.795300006866455,
    0.797803921486970,
    0.800300002098084,
    0.802803892322847,
    0.805299997329712,
    0.807803863460723,
    0.810299992561340,
    0.812803834895027,
    0.815299987792969,
    0.817803806620319,
    0.820299983024597,
    0.822803778631297,
    0.825299978256226,
    0.827803750922783,
    0.830299973487854,
    0.832803753381377,
    0.835300028324127,
    0.837803755931570,
    0.840300023555756,
    0.842803729034748,
    0.845300018787384,
    0.847803702398935,
    0.850300014019012,
    0.852803676019539,
    0.855300009250641,
    0.857803649892077,
    0.860300004482269,
    0.862803624012169,
    0.865299999713897,
];

fn get_level(level: f64) -> usize {
    (level / 0.5) as usize - 2
}

static ETAG: Lazy<ArcSwap<Vec<u8>>> = Lazy::new(Default::default);
static STATS: Lazy<ArcSwap<HashMap<PokemonWithFormAndGender, Stats>>> = Lazy::new(Default::default);
static GREAT_LEAGUE: Lazy<ArcSwap<HashMap<PokemonWithFormAndGender, Arc<Vec<Response>>>>> = Lazy::new(Default::default);
static ULTRA_LEAGUE: Lazy<ArcSwap<HashMap<PokemonWithFormAndGender, Arc<Vec<Response>>>>> = Lazy::new(Default::default);

fn normalize<S: AsRef<str>>(s: S) -> String {
    s.as_ref().to_lowercase().replace('_', " ")
}

#[derive(Debug, Hash, Eq, PartialEq)]
struct PokemonWithFormAndGender {
    pub pokemon: String,
    pub form: Option<String>,
    pub gender: Option<Gender>,
}

impl TryFrom<&EvolutionBranch> for PokemonWithFormAndGender {
    type Error = ();
    fn try_from(eb: &EvolutionBranch) -> Result<Self, Self::Error> {
        Ok(PokemonWithFormAndGender {
            pokemon: eb.evolution.as_deref().map(normalize).ok_or(())?,
            form: eb.form.as_deref().map(normalize),
            gender: eb.gender_requirement.as_deref().map(Gender::from_str),
        })
    }
}

impl From<&PokemonSettings> for PokemonWithFormAndGender {
    fn from(p: &PokemonSettings) -> Self {
        PokemonWithFormAndGender {
            pokemon: normalize(&p.unique_id),
            form: p.form.as_deref().map(normalize),
            gender: None,
        }
    }
}

struct Stats {
    stats: pogo_gamemaster_entities::Stats,
    evolutions: Option<Vec<PokemonWithFormAndGender>>,
}

impl From<&PokemonSettings> for Stats {
    fn from(p: &PokemonSettings) -> Self {
        Stats {
            stats: p.stats,
            evolutions: p.evolution_branch.as_ref().map(|ebs| {
                ebs.iter().filter_map(|eb| eb.try_into().ok()).collect()
            }),
        }
    }
}

#[derive(Debug, Default)]
struct Response {
    pub rank: u32,
    pub percentage: f64,
    pub ivs: Vec<PvpIV>,
}

#[derive(Debug, Clone)]
struct PvpIV {
    pub iv: IV,
    pub level: f64,
    pub cp: u16,
}

/*
    private func loadMasterFile() {
        Log.debug(message: "[PVPStatsManager] Loading game master file")
        let request = CURLRequest("https://raw.githubusercontent.com/PokeMiners/" +
                                  "game_masters/master/latest/latest.json")
        guard let result = try? request.perform() else {
            Log.error(message: "[PVPStatsManager] Failed to load game master file")
            return
        }
        eTag = result.get(.eTag)
        Log.debug(message: "[PVPStatsManager] Parsing game master file")
        let bodyJSON = try? JSONSerialization.jsonObject(with: Data(result.bodyBytes))
        guard let templates = bodyJSON as? [[String: Any]] else {
            Log.error(message: "[PVPStatsManager] Failed to parse game master file")
            return
        }
        var stats = [PokemonWithFormAndGender: Stats]()
        templates.forEach { (template) in
            guard let data = template["data"] as? [String: Any] else { return }
            guard let templateId = data["templateId"] as? String else { return }
            if templateId.starts(with: "V"), templateId.contains(string: "_POKEMON_"),
                let pokemonInfo = data["pokemonSettings"] as? [String: Any],
                let pokemonName = pokemonInfo["pokemonId"] as? String,
                let statsInfo = pokemonInfo["stats"] as? [String: Any],
                let baseStamina = statsInfo["baseStamina"] as? Int,
                let baseAttack = statsInfo["baseAttack"] as? Int,
                let baseDefense = statsInfo["baseDefense"] as? Int {
                guard let pokemon = pokemonFrom(name: pokemonName) else {
                    Log.warning(message: "[PVPStatsManager] Failed to get pokemon for: \(pokemonName)")
                    return
                }
                let formName = pokemonInfo["form"] as? String
                let form: PokemonDisplayProto.Form?
                if let formName = formName {
                    guard let formT = formFrom(name: formName) else {
                        Log.warning(message: "[PVPStatsManager] Failed to get form for: \(formName)")
                        return
                    }
                    form = formT
                } else {
                    form = nil
                }
                var evolutions = [PokemonWithFormAndGender]()
                let evolutionsInfo = pokemonInfo["evolutionBranch"] as? [[String: Any]] ?? []
                for info in evolutionsInfo {
                    if let pokemonName = info["evolution"] as? String, let pokemon = pokemonFrom(name: pokemonName) {
                        let formName = info["form"] as? String
                        let genderName = info["genderRequirement"] as? String
                        let form = formName == nil ? nil : formFrom(name: formName!)
                        let gender = genderName == nil ? nil : genderFrom(name: genderName!)
                        evolutions.append(.init(pokemon: pokemon, form: form, gender: gender))
                    }
                }
                let stat = Stats(baseAttack: baseAttack, baseDefense: baseDefense,
                                  baseStamina: baseStamina, evolutions: evolutions)
                stats[.init(pokemon: pokemon, form: form)] = stat
            }
        }
        rankingGreatLock.lock()
        rankingUltraLock.lock()
        self.stats = stats
        self.rankingGreat = [:]
        self.rankingUltra = [:]
        rankingGreatLock.unlock()
        rankingUltraLock.unlock()
        Log.debug(message: "[PVPStatsManager] Done parsing game master file")
    }
*/
async fn load_master_file() -> Result<(), ()> {
    let start = SystemTime::now();

    let etag = ETAG.load();
    let res = reqwest::get("https://raw.githubusercontent.com/PokeMiners/game_masters/master/latest/latest.json")
        .await
        .map_err(|e| error!("GameMaster retrieve error: {}", e))?;

    let etag = if let Some(header) = res.headers().get("eTag") {
        if etag.as_ref() == header.as_ref() {
            debug!("Skipping update because etag equals to last");
            return Ok(());
        }

        Some(header.as_ref().to_owned())
    }
    else {
        None
    };

    let root = res.json::<Vec<TemplateWrapper>>()
        .await
        .map_err(|e| error!("GameMaster decode error: {}", e))?;

    let stats = root.iter()
        .filter_map(|t| t.data.pokemon.as_ref())
        .map(|p| (p.into(), p.into()))
        .collect();

    let helper = PvpHelper {
        stats: &stats,
    };
    let cache = root.iter()
        .filter_map(|t| t.data.pokemon.as_ref())
        .map(|p| (p.into(), Arc::new(helper._get_top_pvp(&p.unique_id, p.form.as_deref(), League::Great))))
        .collect();
    GREAT_LEAGUE.swap(Arc::new(cache));
    let cache = root.iter()
        .filter_map(|t| t.data.pokemon.as_ref())
        .map(|p| (p.into(), Arc::new(helper._get_top_pvp(&p.unique_id, p.form.as_deref(), League::Ultra))))
        .collect();
    ULTRA_LEAGUE.swap(Arc::new(cache));

    STATS.swap(Arc::new(stats));

    if let Some(header) = etag {
        ETAG.swap(Arc::new(header));
    }

    debug!("master file loaded in {}s", start.elapsed().unwrap_or_default().as_secs_f64());

    Ok(())
}

pub fn init() {
    tokio::spawn(async {
        let mut interval = interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            load_master_file().await.ok();
        }
    });
}

#[derive(Debug, Copy, Clone, PartialEq)]
struct IV {
    pub attack: u8,
    pub defense: u8,
    pub stamina: u8,
}

fn iv_all() -> impl Iterator<Item=IV> {
    (0..=15).map(|attack| (0..=15).map(move |defense| (0..=15).map(move |stamina| IV {
        attack,
        defense,
        stamina,
    })).flatten()).flatten()
}

#[derive(Debug, Copy, Clone)]
enum League {
    Great,
    Ultra,
}

impl League {
    const fn get_cap(&self) -> u16 {
        match self {
            League::Great => 1500,
            League::Ultra => 2500,
        }
    }
}

fn pvp_ranking<PC, FC>(pokemon: &crate::Pokemon, league: League) -> Pvp
where
    PC: Cache<Id=u16>,
    FC: Cache<Id=u16>,
{
    let iv = IV {
        attack: pokemon.individual_attack?,
        defense: pokemon.individual_defense?,
        stamina: pokemon.individual_stamina?,
    };
    let level = pokemon.pokemon_level?.into();

    let name = PC::get(pokemon.pokemon_id)?;
    let form = pokemon.form.and_then(FC::get);
    let costume = pokemon.costume.and_then(FC::get);

    let pvp = PvpHelper::get_pvp_stats_with_evolutions(&name, form.as_deref(), pokemon.gender, costume.as_deref(), iv, level, league);
    if pvp.is_empty() {
        None
    }
    else {
        Some(pvp.into_iter().filter_map(|(p, r)| Some(PvpRanking {
            pokemon: PC::reverse(&p.pokemon)?,
            form: p.form.as_deref().and_then(FC::reverse),
            gender: p.gender,
            rank: Some(r.rank as u16),
            percentage: Some(r.percentage),
            cp: r.ivs.iter().map(|iv| iv.cp).next(),
            level: r.ivs.iter().map(|iv| iv.level as f32).next()
        })).collect())
    }
}

struct PvpHelper<'a> {
    stats: &'a HashMap<PokemonWithFormAndGender, Stats>,
}

impl<'a> PvpHelper<'a> {
    /*
    internal func getPVPStats(pokemon: HoloPokemonId, form: PokemonDisplayProto.Form?,
                              iv: IV, level: Double, league: League) -> Response? {
        guard let stats = getTopPVP(pokemon: pokemon, form: form, league: league) else {
            return nil
        }
        guard let index = stats.firstIndex(where: { value in
            for ivlevel in value.ivs where ivlevel.iv == iv && ivlevel.level >= level {
                return true
            }
            return false
        }) else {
            return nil
        }
        let max = Double(stats[0].rank)
        let result = stats[index]
        let value = Double(result.rank)
        let ivs: [Response.IVWithCP]
        if let currentIV = result.ivs.first(where: { return $0.iv == iv }) {
            ivs = [currentIV]
        } else {
            ivs = []
        }
        return .init(rank: index + 1, percentage: value/max, ivs: ivs)
    }
    */
    fn get_pvp_stats(pokemon: &str, form: Option<&str>, iv: IV, level: f64, league: League) -> Option<Response> {
        let stats = Self::get_top_pvp(pokemon, form, league);
        let index = stats.iter().position(|value| {
            value.ivs.iter().any(|ivlevel| {
                ivlevel.iv == iv && ivlevel.level >= level
            })
        })?;
        let max = stats[0].rank as f64;
        let value = stats[index].rank as f64;
        Some(Response {
            rank: (index as u32) + 1,
            percentage: value / max,
            ivs: stats[index].ivs.iter().find_map(|ivlevel| (ivlevel.iv == iv).then(|| vec![ivlevel.clone()])).unwrap_or_default(),
        })
    }

    /*
    internal func getPVPStatsWithEvolutions(pokemon: HoloPokemonId, form: PokemonDisplayProto.Form?,
                                            gender: PokemonDisplayProto.Gender?,
                                            costume: PokemonDisplayProto.Costume, iv: IV, level: Double, league: League)
                                            -> [(pokemon: PokemonWithFormAndGender, response: Response?)] {
        let current = getPVPStats(pokemon: pokemon, form: form, iv: iv, level: level, league: league)
        var result = [(
                pokemon: PokemonWithFormAndGender(pokemon: pokemon, form: form, gender: gender),
                response: current
        )]
        guard !String(describing: costume).lowercased().contains(string: "noevolve"),
              let stat = stats[.init(pokemon: pokemon, form: form)],
              !stat.evolutions.isEmpty else {
            return result
        }
        for evolution in stat.evolutions {
            if evolution.gender == nil || evolution.gender == gender {
                let pvpStats = getPVPStatsWithEvolutions(
                        pokemon: evolution.pokemon, form: evolution.form,
                        gender: gender, costume: costume, iv: iv, level: level, league: league
                )
                result += pvpStats
            }
        }
        return result
    }
    */
    fn get_pvp_stats_with_evolutions(pokemon: &str, form: Option<&str>, gender: Gender, costume: Option<&str>, iv: IV, level: f64, league: League) -> Vec<(PokemonWithFormAndGender, Response)> {
        let current = Self::get_pvp_stats(pokemon, form, iv, level, league);
        let mut result = if let Some(c) = current {
            vec![(
                PokemonWithFormAndGender {
                    pokemon: normalize(pokemon),
                    form: form.map(normalize),
                    gender: Some(gender),
                },
                c
            )]
        }
        else {
            vec![]
        };
        if form.map(|s| s.contains("noevolve")) == Some(true) {
            return result;
        }

        let index = PokemonWithFormAndGender {
            pokemon: normalize(pokemon),
            form: form.map(normalize),
            gender: None,
        };
        let stats = STATS.load();
        let stat = &stats[&index];
        if let Some(evolutions) = &stat.evolutions {
            for evolution in evolutions {
                if evolution.gender.is_none() || evolution.gender == Some(gender) {
                    result.extend(Self::get_pvp_stats_with_evolutions(&evolution.pokemon, form, gender, costume, iv, level, league));
                }
            }
        }
        result
    }

    /*
    // swiftlint:disable:next cyclomatic_complexity
    internal func getTopPVP(pokemon: HoloPokemonId, form: PokemonDisplayProto.Form?,
                            league: League) -> [Response]? {
        let info = PokemonWithFormAndGender(pokemon: pokemon, form: form)
        let cached: ResponsesOrEvent?
        switch league {
        case .great:
            rankingGreatLock.lock()
            cached = rankingGreat[info]
            rankingGreatLock.unlock()
        case .ultra:
            rankingUltraLock.lock()
            cached = rankingUltra[info]
            rankingUltraLock.unlock()
        }

        if cached == nil {
            switch league {
            case .great:
                rankingGreatLock.lock()
            case .ultra:
                rankingUltraLock.lock()
            }
            guard let stats = stats[info] else {
                switch league {
                case .great:
                    rankingGreatLock.unlock()
                case .ultra:
                    rankingUltraLock.unlock()
                }
                return nil
            }
            let event = Threading.Event()
            switch league {
            case .great:
                rankingGreat[info] = .event(event: event)
                rankingGreatLock.unlock()
            case .ultra:
                rankingUltra[info] = .event(event: event)
                rankingUltraLock.unlock()
            }
            let values = getPVPValuesOrdered(stats: stats, cap: league.rawValue)
            switch league {
            case .great:
                rankingGreatLock.lock()
                rankingGreat[info] = .responses(responses: values)
                rankingGreatLock.unlock()
            case .ultra:
                rankingUltraLock.lock()
                rankingUltra[info] = .responses(responses: values)
                rankingUltraLock.unlock()
            }
            event.lock()
            event.broadcast()
            event.unlock()
            return values
        }
        switch cached! {
        case .responses(let responses):
            return responses
        case .event(let event):
            event.lock()
            _ = event.wait(seconds: 10)
            event.unlock()
            return getTopPVP(pokemon: pokemon, form: form, league: league)
        }
    }
    */
    fn _get_top_pvp(&self, pokemon: &str, form: Option<&str>, league: League) -> Vec<Response> {
        let info = PokemonWithFormAndGender {
            pokemon: normalize(pokemon),
            form: form.map(normalize),
            gender: None,
        };

        self.get_pvp_values_ordered(&self.stats[&info], league.get_cap())
    }
    fn get_top_pvp(pokemon: &str, form: Option<&str>, league: League) -> Arc<Vec<Response>> {
        let info = PokemonWithFormAndGender {
            pokemon: normalize(pokemon),
            form: form.map(normalize),
            gender: None,
        };

        let cache = match league {
            League::Great => GREAT_LEAGUE.load(),
            League::Ultra => ULTRA_LEAGUE.load(),
        };
        Arc::clone(&cache[&info])
    }

    /*
    private func getPVPValuesOrdered(stats: Stats, cap: Int?) -> [Response] {
        var ranking = [Int: Response]()
        for iv in IV.all {
            var maxLevel: Double = 0
            var maxCP: Int = 0
            for level in stride(from: 0.0, through: 50.0, by: 0.5).reversed() {
                let cp = (cap == nil ? 0 : getCPValue(iv: iv, level: level, stats: stats))
                if cp <= (cap ?? 0) {
                    maxLevel = level
                    maxCP = cp
                    break
                }
            }
            if maxLevel != 0 {
                let value = getPVPValue(iv: iv, level: maxLevel, stats: stats)
                if ranking[value] == nil {
                    ranking[value] = Response(rank: value, percentage: 0.0, ivs: [])
                }
                ranking[value]!.ivs.append(.init(iv: iv, level: maxLevel, cp: maxCP))
            }
        }
        return ranking.sorted { (lhs, rhs) -> Bool in
            return lhs.key >= rhs.key
        }.map { (value) -> Response in
            return value.value
        }
    }
    */
    fn get_pvp_values_ordered(&self, stats: &Stats, cap: u16) -> Vec<Response> {
        let mut ranking = BTreeMap::new();
        for iv in iv_all() {
            let mut max_level = 0.0;
            let mut max_cp = 0;
            let mut level = 50.0;
            while level >= 0.0 {
                let cp = self.get_cp_value(iv, level, stats);
                if cp <= cap {
                    max_level = level;
                    max_cp = cp;
                    break;
                }
                level -= 0.5;
            }
            if max_level > 0.0 {
                let value = self.get_pvp_value(iv, max_level, stats);
                let rank = ranking.entry(value).or_insert_with(|| Response {
                    rank: value,
                    percentage: 0.0,
                    ivs: Vec::new(),
                });
                rank.ivs.push(PvpIV {
                    iv,
                    level,
                    cp: max_cp,
                });
            }
        }
        ranking.into_iter().rev().map(|(_, r)| r).collect()
    }

    /*
    private func getPVPValue(iv: IV, level: Double, stats: Stats) -> Int {
        let mutliplier = (PVPStatsManager.cpMultiplier[level] ?? 0)
        let attack = Double(iv.attack + stats.baseAttack) * mutliplier
        let defense = Double(iv.defense + stats.baseDefense) * mutliplier
        let stamina = Double(iv.stamina + stats.baseStamina) * mutliplier
        return Int(round(attack * defense * floor(stamina)))
    }
    */
    fn get_pvp_value(&self, iv: IV, level: f64, stats: &Stats) -> u32 {
        let multiplier = CP_MULTIPLIERS[get_level(level)];
        let attack = ((stats.stats.base_attack + iv.attack as u16) as f64) * multiplier;
        let defense = ((stats.stats.base_defense + iv.defense as u16) as f64) * multiplier;
        let stamina = ((stats.stats.base_stamina + iv.stamina as u16) as f64) * multiplier;
        (attack * defense * stamina.floor()).round() as u32
    }

    /*
    private func getCPValue(iv: IV, level: Double, stats: Stats) -> Int {
        let attack = Double(stats.baseAttack + iv.attack)
        let defense = pow(Double(stats.baseDefense + iv.defense), 0.5)
        let stamina =  pow(Double(stats.baseStamina + iv.stamina), 0.5)
        let multiplier = pow((PVPStatsManager.cpMultiplier[level] ?? 0), 2)
        return max(Int(floor(attack * defense * stamina * multiplier / 10)), 10)
    }
    */
    fn get_cp_value(&self, iv: IV, level: f64, stats: &Stats) -> u16 {
        let attack = (stats.stats.base_attack + iv.attack as u16) as f64;
        let defense = ((stats.stats.base_defense + iv.defense as u16) as f64).powf(0.5);
        let stamina = ((stats.stats.base_stamina + iv.stamina as u16) as f64).powf(0.5);
        let multiplier = CP_MULTIPLIERS[get_level(level)].powi(2);
        ((attack * defense * stamina * multiplier / 10.0).floor() as u16).max(10)
    }
}

#[derive(Clone, Debug)]
pub struct PokemonWithPvpInfo<PC, FC> {
    inner: crate::Pokemon,
    _pc: PhantomData<PC>,
    _fc: PhantomData<FC>,
}

impl<PC, FC> Deref for PokemonWithPvpInfo<PC, FC> {
    type Target = crate::Pokemon;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<PC, FC> From<crate::Pokemon> for PokemonWithPvpInfo<PC, FC> {
    fn from(inner: crate::Pokemon) -> Self {
        PokemonWithPvpInfo {
            inner,
            _pc: PhantomData,
            _fc: PhantomData,
        }
    }
}

impl<'de, PC, FC> Deserialize<'de> for PokemonWithPvpInfo<PC, FC>
where
    PC: Cache<Id=u16>,
    FC: Cache<Id=u16>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut pokemon = crate::Pokemon::deserialize(deserializer)?;
        if pokemon.pvp_rankings_great_league.is_none() && pokemon.pvp_rankings_ultra_league.is_none() {
            pokemon.pvp_rankings_great_league = pvp_ranking::<PC, FC>(&pokemon, League::Great);
            pokemon.pvp_rankings_ultra_league = pvp_ranking::<PC, FC>(&pokemon, League::Ultra);
        }
        Ok(PokemonWithPvpInfo {
            inner: pokemon,
            _pc: PhantomData,
            _fc: PhantomData,
        })
    }
}

pub trait Cache: std::fmt::Debug {
    type Id;
    fn get(id: Self::Id) -> Option<String>;
    fn reverse(name: &str) -> Option<Self::Id>;
}

#[cfg(test)]
mod tests {
    #[derive(Debug)]
    struct FakeCache;

    impl crate::gamemaster::Cache for FakeCache {
        type Id = u16;
        fn get(id: Self::Id) -> Option<String> {
            match id {
                255 => Some(String::from("torchic")),
                _ => None,
            }
        }
        fn reverse(name: &str) -> Option<Self::Id> {
            match name {
                "torchic" => Some(255),
                "combusken" => Some(256),
                _ => None,
            }
        }
    }

    #[tokio::test]
    async fn it_works() {
        tracing_subscriber::fmt::try_init().ok();
        super::load_master_file().await.unwrap();

        let p: crate::Pokemon = serde_json::from_str(r#"{"individual_attack":6,"weight":1.804092288017273,"costume":0,"latitude":45.59854125668628,"weather":1,"pokestop_id":"d558c850604f41d5997a5adb37b366c0.16","display_pokemon_id":null,"encounter_id":"7657693126371544079","disappear_time":1599018063,"cp":890,"last_modified_time":1599017290,"pokemon_id":255,"capture_1":0.14569318294525146,"move_2":63,"capture_2":0.2103751301765442,"gender":1,"username":"Sot20aB0rn","spawnpoint_id":"68C62B17","form":0,"pokemon_level":33,"individual_stamina":9,"pvp_rankings_great_league":[{"rank":809,"percentage":0.8700643398554556,"level":40.0,"form":0,"cp":989,"pokemon":255},{"rank":351,"percentage":0.9607850542156611,"level":38.5,"pokemon":256,"form":0,"cp":1490},{"pokemon":257,"form":0,"percentage":null,"cp":null,"level":null,"rank":null}],"capture_3":0.27015984058380127,"shiny":false,"longitude":8.866217271528695,"is_event":false,"move_1":209,"first_seen":1599017274,"individual_defense":12,"disappear_time_verified":true,"pvp_rankings_ultra_league":[{"cp":989,"level":40.0,"pokemon":255,"form":0,"rank":809,"percentage":0.8700643398554556},{"form":0,"percentage":0.8929189209843369,"cp":1523,"level":40.0,"pokemon":256,"rank":684},{"pokemon":257,"percentage":0.9690298527207762,"form":0,"level":35.0,"cp":2489,"rank":558}],"height":0.2996796667575836}"#).unwrap();
        assert_eq!(p.pvp_rankings_great_league, super::pvp_ranking::<FakeCache, FakeCache>(&p, super::League::Great));
        assert_eq!(p.pvp_rankings_ultra_league, super::pvp_ranking::<FakeCache, FakeCache>(&p, super::League::Ultra));
    }
}
