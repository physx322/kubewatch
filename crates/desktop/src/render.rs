//! Réglage du rendu de la fenêtre : accélération matérielle.
//!
//! WebKitGTK lit son environnement à l'initialisation, bien avant que l'état
//! applicatif n'existe. Ce réglage vit donc dans son propre fichier,
//! `<dossier d'état>/render.json`, relu au tout début de `main` — et il ne
//! prend effet qu'au démarrage suivant.
//!
//! Depuis WebKitGTK 2.46, deux étages se règlent séparément :
//!
//! * la *composition* — assembler les couches de la page et les présenter à
//!   l'écran, toujours accélérée quand le transport de tampons est là ;
//! * la *peinture* — dessiner le contenu des tuiles, confiée à Skia, sur le
//!   processeur, sur la carte graphique, ou les deux (mode « hybride »).
//!
//! C'est la peinture qui pèse : elle fait l'essentiel du rendu d'une page. Les
//! modes vont donc du plus accéléré au plus compatible :
//!
//! * [`Acceleration::Full`] — tout à la carte graphique : composition, et
//!   chaque tuile peinte par le GPU, qui ne partage plus rien avec le
//!   processeur — sa file d'attente absorbe alors les pointes de peinture, ce
//!   qui peut se sentir sur une carte modeste ;
//! * [`Acceleration::Auto`] (défaut) — composition accélérée, peinture au GPU
//!   d'abord et débordements au processeur : l'équilibre que le port GTK règle
//!   lui-même ;
//! * [`Acceleration::CpuPainting`] — composition accélérée, peinture au
//!   processeur : un repli pour les pilotes qui peignent mal, pas pour le
//!   texte — mesure faite, les glyphes sortent au pixel près identiques des
//!   deux chemins, anticrénelage sous-pixel compris ;
//! * [`Acceleration::Off`] — rendu logiciel, dernier recours pour une fenêtre
//!   noire ou des artefacts graphiques.
//!
//! `WEBKIT_DISABLE_DMABUF_RENDERER` n'est plus posée que pour [`Acceleration::Off`] :
//! depuis WebKitGTK 2.42 elle ne donne pas « l'accélération sans DMA-BUF » mais
//! plus d'accélération du tout — sans transport de tampons,
//! `AcceleratedBackingStore::checkRequirements()` échoue, la composition
//! accélérée est abandonnée et le processus web renonce aux tampons GPU.
//!
//! Les variables posées dans l'environnement gardent le dernier mot : une
//! distribution ou un utilisateur qui en impose une n'est jamais contredit.
//!
//! Ce module règle aussi la *rastérisation du texte* — [`TextRendering`] —,
//! qui ne doit rien à l'accélération : WebKitGTK la prend dans `GtkSettings`,
//! avant que la fenêtre ne naisse. Elle attend elle aussi le démarrage suivant,
//! car le processus web garde les polices qu'il a déjà construites : mesure
//! faite, ni un changement de réglage ni un rechargement de la page ne les
//! refont.

use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Nom du fichier dans le dossier d'état.
pub const FILE_NAME: &str = "render.json";

/// Désactive l'import des tampons par DMA-BUF, donc toute l'accélération.
const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
/// Désactive la composition accélérée : tout repasse par le processeur.
const COMPOSITING: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";
/// Cantonne la peinture Skia au processeur ; la composition reste accélérée.
const CPU_RENDERING: &str = "WEBKIT_SKIA_ENABLE_CPU_RENDERING";
/// Répartition de la peinture entre processeur et carte graphique.
const HYBRID_STRATEGY: &str = "WEBKIT_SKIA_HYBRID_PAINTING_MODE_STRATEGY";
/// Part des tuiles confiées au GPU, en pourcentage, pour la stratégie
/// `MinimumFractionOfTasksUsingGPU`.
const GPU_FRACTION: &str = "WEBKIT_SKIA_GPU_MIN_FRACTION_OF_TASKS_IN_PERCENT";

/// Les variables que ce module gère : l'une d'elles déjà posée, et il s'efface.
const MANAGED: [&str; 5] = [
    DMABUF,
    COMPOSITING,
    CPU_RENDERING,
    HYBRID_STRATEGY,
    GPU_FRACTION,
];

/// Ce que l'appliqué de cette session a retenu, pour le dire à l'interface.
static APPLIED: OnceLock<(Applied, bool)> = OnceLock::new();

/// Rastérisation réellement en vigueur depuis le lancement.
static APPLIED_TEXT: OnceLock<TextRendering> = OnceLock::new();

/// Choix de l'utilisateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Acceleration {
    /// Composition accélérée, peinture au GPU puis au processeur.
    #[default]
    Auto,
    /// Composition et peinture entièrement à la carte graphique.
    Full,
    /// Composition accélérée, peinture au processeur.
    CpuPainting,
    /// Rendu logiciel.
    Off,
}

/// Rendu réellement en vigueur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Applied {
    /// Tout le rendu à la carte graphique.
    Gpu,
    /// Composition accélérée, peinture partagée GPU et processeur.
    Hybrid,
    /// Composition accélérée, peinture au processeur.
    CpuPainting,
    /// DMA-BUF coupé par l'environnement : plus d'accélération.
    NoDmabuf,
    /// Rendu logiciel.
    Software,
}

/// Rastérisation du texte : comment les glyphes sont posés sur la grille de
/// pixels.
///
/// WebKitGTK lit `GtkSettings` au démarrage du processus web ; le choix prend
/// effet au lancement suivant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextRendering {
    /// Ce que règle le bureau, sans y toucher.
    #[default]
    System,
    /// Anticrénelage sous-pixel RGB et hinting complet : le plus net sur un
    /// écran LCD à bandes RGB, et le plus lisible en dessous de 100 ppp.
    Sharp,
    /// Niveaux de gris et hinting léger : formes fidèles, bords plus doux, et
    /// rien à craindre d'une dalle dont l'ordre des sous-pixels diffère.
    Smooth,
}

/// Réglages tels que persistés.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderSettings {
    /// Accélération matérielle demandée.
    pub acceleration: Acceleration,
    /// Rastérisation du texte demandée.
    pub text: TextRendering,
}

/// Réglages tels que présentés à l'interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderView {
    /// Choix enregistré.
    pub acceleration: Acceleration,
    /// Rastérisation du texte enregistrée.
    pub text: TextRendering,
    /// Ce qui est en vigueur depuis le lancement.
    pub applied: Applied,
    /// Vrai si l'environnement impose le mode : le choix est alors sans effet.
    pub forced_by_env: bool,
    /// Vrai si le choix enregistré attend un redémarrage pour s'appliquer.
    pub restart_needed: bool,
    /// Vrai si la plateforme sait faire quelque chose de ce réglage.
    pub supported: bool,
}

/// Vrai là où ces variables ont un sens : WebKitGTK, donc Linux et BSD.
pub const SUPPORTED: bool = cfg!(all(unix, not(target_os = "macos")));

/// Applique le réglage et renvoie le mode retenu.
///
/// À appeler en tout premier dans `main` : GTK et WebKit lisent ces variables
/// à leur initialisation, et l'écriture doit précéder le démarrage des fils.
pub fn apply(state_dir: &Path) -> Applied {
    let forced = MANAGED.iter().any(|v| std::env::var_os(v).is_some());
    let applied = if forced || !SUPPORTED {
        from_env()
    } else {
        let target = target_of(load(state_dir).acceleration);
        for (name, value) in vars_for(target) {
            std::env::set_var(name, value);
        }
        target
    };
    let _ = APPLIED.set((applied, forced));
    applied
}

/// Mode en vigueur, et vrai si c'est l'environnement qui l'impose.
///
/// Avant [`apply`] — donc jamais, en pratique — on répond la peinture partagée,
/// qui est le comportement du port GTK sans rien lui dire.
pub fn current() -> (Applied, bool) {
    *APPLIED.get().unwrap_or(&(Applied::Hybrid, false))
}

/// Ce que l'interface affiche.
pub fn view(state_dir: &Path) -> RenderView {
    let (applied, forced_by_env) = current();
    let RenderSettings { acceleration, text } = load(state_dir);
    RenderView {
        acceleration,
        text,
        applied,
        forced_by_env,
        restart_needed: (SUPPORTED && !forced_by_env && target_of(acceleration) != applied)
            || text != applied_text(),
        supported: SUPPORTED,
    }
}

/// Enregistre le choix ; il s'appliquera au démarrage suivant.
pub fn set_acceleration(
    state_dir: &Path,
    acceleration: Acceleration,
) -> Result<RenderView, String> {
    save(
        state_dir,
        RenderSettings {
            acceleration,
            ..load(state_dir)
        },
    )?;
    Ok(view(state_dir))
}

/// Enregistre la rastérisation du texte ; l'appeler n'applique rien, c'est le
/// rôle d'[`apply_text`], qui a besoin du fil principal.
pub fn set_text_rendering(state_dir: &Path, text: TextRendering) -> Result<RenderView, String> {
    save(
        state_dir,
        RenderSettings {
            text,
            ..load(state_dir)
        },
    )?;
    Ok(view(state_dir))
}

/// Mode correspondant à un choix.
fn target_of(acceleration: Acceleration) -> Applied {
    match acceleration {
        Acceleration::Auto => Applied::Hybrid,
        Acceleration::Full => Applied::Gpu,
        Acceleration::CpuPainting => Applied::CpuPainting,
        Acceleration::Off => Applied::Software,
    }
}

/// Variables à poser pour obtenir un mode.
///
/// Rien à poser pour la composition accélérée ni pour le DMA-BUF : WebKitGTK
/// les prend par défaut, et sa politique d'accélération vaut déjà « toujours ».
fn vars_for(applied: Applied) -> &'static [(&'static str, &'static str)] {
    match applied {
        // Toute la peinture au GPU : la part minimale de tuiles qui lui
        // reviennent est portée à 100 %, le processeur ne sert plus que si
        // l'accélération manque à l'appel.
        Applied::Gpu => &[
            (HYBRID_STRATEGY, "MinimumFractionOfTasksUsingGPU"),
            (GPU_FRACTION, "100"),
        ],
        // GPU d'abord, processeur pour les débordements : le défaut du port
        // GTK, posé explicitement pour ne pas dépendre de ce défaut.
        Applied::Hybrid => &[(HYBRID_STRATEGY, "GPUAffineRendering")],
        Applied::CpuPainting => &[(CPU_RENDERING, "1")],
        Applied::Software => &[(CPU_RENDERING, "1"), (DMABUF, "1"), (COMPOSITING, "1")],
        // Celui-là ne vient que de l'environnement.
        Applied::NoDmabuf => &[],
    }
}

/// Réglages de rastérisation du bureau, relevés avant la première retouche :
/// c'est à eux que revient [`TextRendering::System`].
#[cfg(all(unix, not(target_os = "macos")))]
static SYSTEM_TEXT: OnceLock<SystemText> = OnceLock::new();

/// Ce que le bureau demandait au démarrage.
#[cfg(all(unix, not(target_os = "macos")))]
#[derive(Debug, Clone)]
struct SystemText {
    antialias: i32,
    hinting: i32,
    hintstyle: Option<String>,
    rgba: Option<String>,
}

/// Applique la rastérisation du texte.
///
/// À appeler sur le fil principal, et de préférence **avant la création de la
/// fenêtre** : WebKitGTK relit bien ces propriétés à chaud, mais il ne
/// rerastérise pas les polices déjà construites ; ce qui est à l'écran garde
/// son aspect jusqu'à un rechargement de la page.
///
/// `GtkSettings` n'existe qu'une fois GTK debout, d'où l'initialisation ici :
/// `gtk_init` ne fait rien la seconde fois, et tao peut la refaire sans
/// dommage quand il monte sa fenêtre.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn apply_text(text: TextRendering) {
    use gtk::prelude::*;

    if let Err(e) = gtk::init() {
        tracing::warn!(erreur = %e, "GTK indisponible : rastérisation du texte inchangée");
        let _ = APPLIED_TEXT.set(TextRendering::System);
        return;
    }
    let Some(settings) = gtk::Settings::default() else {
        tracing::warn!("réglages GTK absents : rastérisation du texte inchangée");
        let _ = APPLIED_TEXT.set(TextRendering::System);
        return;
    };
    let origine = SYSTEM_TEXT.get_or_init(|| SystemText {
        antialias: settings.gtk_xft_antialias(),
        hinting: settings.gtk_xft_hinting(),
        hintstyle: settings.gtk_xft_hintstyle().map(|v| v.to_string()),
        rgba: settings.gtk_xft_rgba().map(|v| v.to_string()),
    });

    let (antialias, hinting, hintstyle, rgba) = match text {
        TextRendering::System => (
            origine.antialias,
            origine.hinting,
            origine.hintstyle.clone(),
            origine.rgba.clone(),
        ),
        TextRendering::Sharp => (1, 1, Some("hintfull".to_owned()), Some("rgb".to_owned())),
        TextRendering::Smooth => (1, 1, Some("hintslight".to_owned()), Some("none".to_owned())),
    };
    settings.set_gtk_xft_antialias(antialias);
    settings.set_gtk_xft_hinting(hinting);
    settings.set_gtk_xft_hintstyle(hintstyle.as_deref());
    settings.set_gtk_xft_rgba(rgba.as_deref());
    let _ = APPLIED_TEXT.set(text);
    tracing::info!(?text, ?hintstyle, ?rgba, "rastérisation du texte");
}

/// Sans WebKitGTK, il n'y a rien à régler.
#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn apply_text(_text: TextRendering) {
    let _ = APPLIED_TEXT.set(TextRendering::System);
}

/// Rastérisation en vigueur depuis le lancement. Avant [`apply_text`], celle
/// du bureau.
pub fn applied_text() -> TextRendering {
    *APPLIED_TEXT.get().unwrap_or(&TextRendering::System)
}

/// Mode déduit des variables déjà posées.
fn from_env() -> Applied {
    if env_flag(COMPOSITING).unwrap_or(false) {
        Applied::Software
    } else if env_flag(DMABUF).unwrap_or(false) {
        Applied::NoDmabuf
    } else if env_flag(CPU_RENDERING).unwrap_or(false) {
        Applied::CpuPainting
    } else {
        Applied::Hybrid
    }
}

/// Variable d'environnement lue comme un drapeau ; absente donne `None`,
/// vide ou `0` donne `Some(false)`.
fn env_flag(name: &str) -> Option<bool> {
    let v = std::env::var(name).ok()?;
    let v = v.trim();
    Some(!(v.is_empty() || v == "0"))
}

/// Réglages enregistrés ; fichier absent ou illisible donne les défauts.
pub fn load(state_dir: &Path) -> RenderSettings {
    let path = state_dir.join(FILE_NAME);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!(fichier = %path.display(), erreur = %e, "réglages de rendu illisibles");
            RenderSettings::default()
        }),
        Err(_) => RenderSettings::default(),
    }
}

/// Écriture atomique : fichier temporaire à côté, puis renommage.
fn save(state_dir: &Path, s: RenderSettings) -> Result<(), String> {
    let path = state_dir.join(FILE_NAME);
    std::fs::create_dir_all(state_dir).map_err(|e| format!("dossier d'état inaccessible : {e}"))?;
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(&s).map_err(|e| format!("sérialisation : {e}"))?;
    std::fs::write(&tmp, &data).map_err(|e| format!("écriture de {} : {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("écriture de {} : {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choix_par_defaut_et_persistance() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).acceleration, Acceleration::Auto);

        set_acceleration(dir.path(), Acceleration::Off).unwrap();
        assert_eq!(load(dir.path()).acceleration, Acceleration::Off);
        set_acceleration(dir.path(), Acceleration::Full).unwrap();
        assert_eq!(load(dir.path()).acceleration, Acceleration::Full);
        set_acceleration(dir.path(), Acceleration::CpuPainting).unwrap();
        assert_eq!(load(dir.path()).acceleration, Acceleration::CpuPainting);

        // Les deux réglages cohabitent : régler l'un ne remet pas l'autre à zéro.
        assert_eq!(load(dir.path()).text, TextRendering::System);
        set_text_rendering(dir.path(), TextRendering::Sharp).unwrap();
        assert_eq!(load(dir.path()).text, TextRendering::Sharp);
        set_acceleration(dir.path(), Acceleration::Auto).unwrap();
        assert_eq!(load(dir.path()).text, TextRendering::Sharp);
        assert_eq!(load(dir.path()).acceleration, Acceleration::Auto);

        // Un fichier abîmé ne bloque pas le démarrage.
        std::fs::write(dir.path().join(FILE_NAME), b"{ pas du json").unwrap();
        assert_eq!(load(dir.path()).acceleration, Acceleration::Auto);

        // Et un fichier écrit par une version plus récente non plus.
        std::fs::write(dir.path().join(FILE_NAME), br#"{"inconnu":1}"#).unwrap();
        assert_eq!(load(dir.path()).acceleration, Acceleration::Auto);
    }

    #[test]
    fn un_texte_non_appliqué_demande_un_redémarrage() {
        // Aucun test n'appelle `apply_text`, donc la rastérisation en vigueur
        // reste celle du bureau : demander autre chose attend un redémarrage.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(applied_text(), TextRendering::System);
        assert!(!view(dir.path()).restart_needed);

        let v = set_text_rendering(dir.path(), TextRendering::Sharp).unwrap();
        assert!(v.restart_needed);
        let v = set_text_rendering(dir.path(), TextRendering::System).unwrap();
        assert!(!v.restart_needed);
    }

    #[test]
    fn correspondance_choix_mode() {
        assert_eq!(target_of(Acceleration::Auto), Applied::Hybrid);
        assert_eq!(target_of(Acceleration::Full), Applied::Gpu);
        assert_eq!(target_of(Acceleration::CpuPainting), Applied::CpuPainting);
        assert_eq!(target_of(Acceleration::Off), Applied::Software);
    }

    #[test]
    fn les_modes_acceleres_laissent_le_gpu_peindre() {
        // Aucun mode accéléré ne coupe la composition, le DMA-BUF ou la
        // peinture GPU : ce sont eux qui rendraient la carte inutile.
        for mode in [Applied::Gpu, Applied::Hybrid] {
            let vars = vars_for(mode);
            for coupure in [DMABUF, COMPOSITING, CPU_RENDERING] {
                assert!(
                    !vars.iter().any(|(n, _)| *n == coupure),
                    "{mode:?} ne devrait pas poser {coupure}"
                );
            }
        }
        // « Tout GPU » envoie bien 100 % des tuiles à la carte.
        let gpu = vars_for(Applied::Gpu);
        assert_eq!(
            gpu.iter()
                .find(|(n, _)| *n == HYBRID_STRATEGY)
                .map(|(_, v)| *v),
            Some("MinimumFractionOfTasksUsingGPU")
        );
        assert_eq!(
            gpu.iter()
                .find(|(n, _)| *n == GPU_FRACTION)
                .map(|(_, v)| *v),
            Some("100")
        );
        // Le rendu logiciel, lui, coupe tout.
        let soft = vars_for(Applied::Software);
        for coupure in [DMABUF, COMPOSITING, CPU_RENDERING] {
            assert!(soft.iter().any(|(n, v)| *n == coupure && *v == "1"));
        }
        // La peinture processeur garde la composition accélérée.
        assert_eq!(vars_for(Applied::CpuPainting), &[(CPU_RENDERING, "1")]);
        // Toutes les variables posées sont des variables gérées, sinon
        // `apply` les écrirait sans jamais s'effacer devant elles.
        for mode in [
            Applied::Gpu,
            Applied::Hybrid,
            Applied::CpuPainting,
            Applied::Software,
        ] {
            for (name, _) in vars_for(mode) {
                assert!(MANAGED.contains(name), "{name} manque à MANAGED");
            }
        }
    }

    #[test]
    fn lecture_des_drapeaux_d_environnement() {
        let nom = "KUBEWATCH_TEST_DRAPEAU";
        std::env::remove_var(nom);
        assert_eq!(env_flag(nom), None);
        std::env::set_var(nom, "");
        assert_eq!(env_flag(nom), Some(false));
        std::env::set_var(nom, "0");
        assert_eq!(env_flag(nom), Some(false));
        std::env::set_var(nom, "1");
        assert_eq!(env_flag(nom), Some(true));
        std::env::remove_var(nom);
    }

    #[test]
    fn le_json_parle_camel_case() {
        let v = RenderView {
            acceleration: Acceleration::CpuPainting,
            text: TextRendering::Sharp,
            applied: Applied::Gpu,
            forced_by_env: false,
            restart_needed: true,
            supported: true,
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""acceleration":"cpuPainting""#));
        assert!(json.contains(r#""text":"sharp""#));
        assert!(json.contains(r#""applied":"gpu""#));
        assert!(json.contains(r#""restartNeeded":true"#));
    }
}
