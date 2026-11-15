use clap::{Parser, Subcommand};
use plano_core::{Id, ItemStatus};
use plano_engine::{Clock, SystemClock};
use plano_store::Store;

#[derive(Parser)]
#[command(name = "plano-cli", version, about = "Plano — planificateur local hors ligne")]
struct Cli {
    /// Chemin du coffre (fichier SQLite)
    #[arg(long, default_value = "plano.db")]
    db: String,
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
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let store = Store::open(&cli.db)?;
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
                println!("{}", if ok { "Tâche terminée." } else { "Tâche introuvable." });
            }
        },
    }
    Ok(())
}
