use clap::{Parser, Subcommand};
use plano_core::schedule::{weekday, Edge, Reminder, ReminderRule, ScheduleItem, Span, Trigger};
use plano_core::{Id, ItemStatus};
use plano_engine::schedule::{catch_up, next_wake, resolve_all, FixedOffset};
use plano_engine::time::{format_local, parse_hhmm, parse_local};
use plano_engine::{Clock, SystemClock};
use plano_store::Store;
use std::collections::HashMap;
use std::time::Duration;

type BoxError = Box<dyn std::error::Error>;

#[derive(Parser)]
#[command(
    name = "plano-cli",
    version,
    about = "Plano — planificateur local hors ligne"
)]
struct Cli {
    /// Chemin du coffre (fichier SQLite)
    #[arg(long, default_value = "plano.db")]
    db: String,
    /// Décalage horaire local en heures par rapport à UTC (ex. 1 pour le Bénin).
    /// Temporaire : la gestion automatique du fuseau viendra avec l'interface.
    #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
    utc_offset: i64,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Gérer les plans
    Plan {
        #[command(subcommand)]
        action: PlanCmd,
    },
    /// Gérer les tâches
    Task {
        #[command(subcommand)]
        action: TaskCmd,
    },
    /// Gérer les rappels
    Reminder {
        #[command(subcommand)]
        action: ReminderCmd,
    },
    /// Afficher les tâches planifiées, dans l'ordre chronologique
    Agenda,
    /// Rattraper les rappels manqués depuis la dernière vérification
    Check,
    /// Attendre et afficher les rappels à l'heure (Ctrl+C pour arrêter)
    Watch,
}

#[derive(Subcommand)]
enum PlanCmd {
    /// Créer un plan
    Add { title: String },
    /// Lister les plans
    List,
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Ajouter une tâche à un plan
    Add { plan: String, title: String },
    /// Lister les tâches d'un plan
    List { plan: String },
    /// Marquer une tâche comme terminée
    Done { id: String },
    /// Planifier une tâche : heure fixe (--at) ou relative à une autre tâche déjà planifiée (--after)
    Schedule {
        id: String,
        /// Heure fixe locale, format "AAAA-MM-JJ HH:MM"
        #[arg(long)]
        at: Option<String>,
        /// Identifiant de la tâche d'ancrage
        #[arg(long)]
        after: Option<String>,
        /// Décalage en minutes par rapport à l'ancre (négatif = avant)
        #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
        offset_min: i64,
        /// Bord de l'ancre : start ou end
        #[arg(long, default_value = "start")]
        edge: String,
        /// Durée en minutes
        #[arg(long, default_value_t = 0)]
        duration_min: i64,
    },
}

#[derive(Subcommand)]
enum ReminderCmd {
    /// Créer un rappel : une option parmi --at, --daily, --before
    Add {
        /// Libellé affiché
        label: String,
        /// Heure fixe locale, "AAAA-MM-JJ HH:MM"
        #[arg(long)]
        at: Option<String>,
        /// Tous les jours à cette heure, "HH:MM"
        #[arg(long)]
        daily: Option<String>,
        /// Jours du rappel quotidien : all, semaine, ou lun,mar,mer,jeu,ven,sam,dim
        #[arg(long, default_value = "all")]
        days: String,
        /// Identifiant d'une tâche planifiée : rappel avant son début
        #[arg(long)]
        before: Option<String>,
        /// Avance en minutes pour --before
        #[arg(long, default_value_t = 15)]
        lead_min: i64,
    },
    /// Lister les rappels
    List,
    /// Supprimer un rappel
    Delete { id: String },
}

/// État chargé depuis le coffre : horaires résolus et rappels.
struct Snapshot {
    spans: HashMap<Id, Span>,
    reminders: Vec<(Reminder, String)>,
}

impl Snapshot {
    fn load(store: &Store) -> Result<Self, BoxError> {
        let spans = resolve_all(&store.list_schedule_items()?)?;
        let reminders = store.list_reminders()?;
        Ok(Snapshot { spans, reminders })
    }

    fn plain(&self) -> Vec<Reminder> {
        self.reminders.iter().map(|(r, _)| r.clone()).collect()
    }

    fn label(&self, id: Id) -> &str {
        self.reminders
            .iter()
            .find(|(r, _)| r.id == id)
            .map(|(_, l)| l.as_str())
            .unwrap_or("?")
    }
}

fn parse_days(s: &str) -> Result<u8, String> {
    let mut mask = 0u8;
    for part in s.split(',').map(|p| p.trim().to_lowercase()) {
        mask |= match part.as_str() {
            "all" | "tous" => weekday::ALL,
            "workdays" | "semaine" => weekday::WORKDAYS,
            "mon" | "lun" => weekday::MON,
            "tue" | "mar" => weekday::TUE,
            "wed" | "mer" => weekday::WED,
            "thu" | "jeu" => weekday::THU,
            "fri" | "ven" => weekday::FRI,
            "sat" | "sam" => weekday::SAT,
            "sun" | "dim" => weekday::SUN,
            other => return Err(format!("jour inconnu : {other}")),
        };
    }
    Ok(mask)
}

fn days_label(mask: u8) -> String {
    let names = ["lun", "mar", "mer", "jeu", "ven", "sam", "dim"];
    names
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1u8 << i) != 0)
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join(",")
}

fn describe_rule(rule: &ReminderRule, off: i64) -> String {
    match *rule {
        ReminderRule::At(t) => format!("le {}", format_local(t, off)),
        ReminderRule::BeforeItem { item, lead_s } => {
            format!("{} min avant la tâche {item}", lead_s / 60)
        }
        ReminderRule::Daily {
            hour,
            minute,
            weekdays,
        } => format!(
            "chaque jour à {hour:02}:{minute:02} ({})",
            days_label(weekdays)
        ),
    }
}

fn main() -> Result<(), BoxError> {
    let cli = Cli::parse();
    let store = Store::open(&cli.db)?;
    let off = cli.utc_offset * 3_600;
    let tz = FixedOffset(off);
    let now = SystemClock.now();

    match cli.cmd {
        Cmd::Plan { action } => match action {
            PlanCmd::Add { title } => {
                let id = store.add_plan(&title, now)?;
                println!("Plan créé : {id}");
            }
            PlanCmd::List => {
                for (id, title) in store.list_plans()? {
                    println!("{id}  {title}");
                }
            }
        },
        Cmd::Task { action } => match action {
            TaskCmd::Add { plan, title } => {
                let id = store.add_item(plan.parse::<Id>()?, &title, now)?;
                println!("Tâche créée : {id}");
            }
            TaskCmd::List { plan } => {
                for it in store.list_items(plan.parse::<Id>()?)? {
                    println!("{}  [{}]  {}", it.id, it.status.as_str(), it.title);
                }
            }
            TaskCmd::Done { id } => {
                let ok = store.set_status(id.parse::<Id>()?, ItemStatus::Termine, now)?;
                println!(
                    "{}",
                    if ok {
                        "Tâche terminée."
                    } else {
                        "Tâche introuvable."
                    }
                );
            }
            TaskCmd::Schedule {
                id,
                at,
                after,
                offset_min,
                edge,
                duration_min,
            } => {
                let id: Id = id.parse()?;
                let trigger = match (at, after) {
                    (Some(a), None) => Trigger::Absolute(
                        parse_local(&a, off)
                            .ok_or("date invalide, format attendu : AAAA-MM-JJ HH:MM")?,
                    ),
                    (None, Some(anchor)) => {
                        let edge = match edge.as_str() {
                            "start" => Edge::Start,
                            "end" => Edge::End,
                            _ => return Err("--edge doit valoir start ou end".into()),
                        };
                        Trigger::RelativeTo {
                            anchor: anchor.parse()?,
                            edge,
                            offset_s: offset_min * 60,
                        }
                    }
                    _ => return Err("indiquez soit --at, soit --after".into()),
                };
                let duration_s = duration_min * 60;

                // Vérifie le graphe avant d'écrire : cycle ou ancre non planifiée = refus.
                let mut items = store.list_schedule_items()?;
                items.retain(|i| i.id != id);
                items.push(ScheduleItem {
                    id,
                    trigger,
                    duration_s,
                });
                let spans = resolve_all(&items)?;

                if store.set_trigger(id, trigger, duration_s, now)? {
                    match spans.get(&id) {
                        Some(s) => println!("Tâche planifiée : {}", format_local(s.start, off)),
                        None => println!("Tâche planifiée."),
                    }
                } else {
                    println!("Tâche introuvable.");
                }
            }
        },
        Cmd::Reminder { action } => match action {
            ReminderCmd::Add {
                label,
                at,
                daily,
                days,
                before,
                lead_min,
            } => {
                let rule = match (at, daily, before) {
                    (Some(a), None, None) => ReminderRule::At(
                        parse_local(&a, off)
                            .ok_or("date invalide, format attendu : AAAA-MM-JJ HH:MM")?,
                    ),
                    (None, Some(d), None) => {
                        let (hour, minute) =
                            parse_hhmm(&d).ok_or("heure invalide, format attendu : HH:MM")?;
                        ReminderRule::Daily {
                            hour,
                            minute,
                            weekdays: parse_days(&days)?,
                        }
                    }
                    (None, None, Some(b)) => ReminderRule::BeforeItem {
                        item: b.parse()?,
                        lead_s: lead_min * 60,
                    },
                    _ => {
                        return Err(
                            "indiquez exactement une option parmi --at, --daily, --before".into(),
                        )
                    }
                };
                let id = store.add_reminder(rule, &label, now)?;
                println!("Rappel créé : {id}");
            }
            ReminderCmd::List => {
                for (r, label) in store.list_reminders()? {
                    println!("{}  {}  ({})", r.id, label, describe_rule(&r.rule, off));
                }
            }
            ReminderCmd::Delete { id } => {
                let ok = store.delete_reminder(id.parse::<Id>()?, now)?;
                println!(
                    "{}",
                    if ok {
                        "Rappel supprimé."
                    } else {
                        "Rappel introuvable."
                    }
                );
            }
        },
        Cmd::Agenda => {
            let snap = Snapshot::load(&store)?;
            let mut rows: Vec<(Id, Span)> = snap.spans.iter().map(|(i, s)| (*i, *s)).collect();
            rows.sort_by_key(|(_, s)| s.start);
            if rows.is_empty() {
                println!("Aucune tâche planifiée.");
            }
            for (id, span) in rows {
                let title = store.item_title(id)?.unwrap_or_default();
                if span.end > span.start {
                    println!(
                        "{} -> {}  {}",
                        format_local(span.start, off),
                        format_local(span.end, off),
                        title
                    );
                } else {
                    println!("{}  {}", format_local(span.start, off), title);
                }
            }
        }
        Cmd::Check => {
            let snap = Snapshot::load(&store)?;
            let last_run = store.last_run()?.unwrap_or(now);
            let missed = catch_up(&snap.plain(), last_run, now, &snap.spans, &tz);
            if missed.is_empty() {
                println!("Aucun rappel manqué.");
            }
            for m in &missed {
                println!(
                    "Manqué : {} (dernier passage {}, {} fois)",
                    snap.label(m.reminder),
                    format_local(m.latest_at, off),
                    m.count
                );
                store.mark_reminder_fired(m.reminder, m.latest_at)?;
            }
            store.set_last_run(now)?;

            let snap = Snapshot::load(&store)?;
            match next_wake(&snap.plain(), now, &snap.spans, &tz) {
                Some((t, id)) => println!(
                    "Prochain réveil : {}  {}",
                    format_local(t, off),
                    snap.label(id)
                ),
                None => println!("Aucun rappel à venir."),
            }
        }
        Cmd::Watch => loop {
            let now = SystemClock.now();
            let snap = Snapshot::load(&store)?;
            match next_wake(&snap.plain(), now, &snap.spans, &tz) {
                None => {
                    println!("Aucun rappel à venir.");
                    break;
                }
                Some((t, id)) => {
                    println!("En attente jusqu'à {} ...", format_local(t, off));
                    let wait = u64::try_from((t - now).max(0)).unwrap_or(0);
                    std::thread::sleep(Duration::from_secs(wait));
                    println!("RAPPEL : {}", snap.label(id));
                    store.mark_reminder_fired(id, t)?;
                    store.set_last_run(t)?;
                }
            }
        },
    }
    Ok(())
}
