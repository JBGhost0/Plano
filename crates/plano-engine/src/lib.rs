//! plano-engine : logique de planification pure (horloge injectée, aucune E/S).
pub mod schedule;

use plano_core::{Chain, ChainState, Deadline, StepState, Ts};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock {
    fn now(&self) -> Ts;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Ts {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// Horloge fixe, utile pour les tests.
pub struct FixedClock(pub Ts);

impl Clock for FixedClock {
    fn now(&self) -> Ts {
        self.0
    }
}

/// Résultat d'une évaluation : l'appelant persiste et notifie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    StepMissed { step: usize },
    StepAutoValidated { step: usize },
    ChainInterrupted,
    ChainCompleted,
}

fn activate(chain: &mut Chain, index: usize, at: Ts) {
    let step = &mut chain.steps[index];
    step.state = StepState::Active;
    step.due_at = Some(match step.deadline {
        Deadline::Absolute(t) => t,
        Deadline::AfterPrevious(d) => at + d,
    });
}

fn advance(chain: &mut Chain, finished: usize, at: Ts, out: &mut Vec<Effect>) {
    let next = finished + 1;
    if next < chain.steps.len() {
        activate(chain, next, at);
        chain.state = ChainState::EnCours;
    } else {
        chain.state = ChainState::Terminee;
        out.push(Effect::ChainCompleted);
    }
}

/// Démarre une suite : la première étape devient active.
pub fn start(chain: &mut Chain, now: Ts) {
    if chain.state != ChainState::NonDemarree {
        return;
    }
    if chain.steps.is_empty() {
        chain.state = ChainState::Terminee;
        return;
    }
    activate(chain, 0, now);
    chain.state = ChainState::EnCours;
}

/// Marque l'étape active comme terminée et passe à la suivante.
pub fn complete_current(chain: &mut Chain, now: Ts) -> Vec<Effect> {
    let mut out = Vec::new();
    if chain.state != ChainState::EnCours {
        return out;
    }
    if let Some(i) = chain
        .steps
        .iter()
        .position(|s| s.state == StepState::Active)
    {
        chain.steps[i].state = StepState::Terminee;
        advance(chain, i, now, &mut out);
    }
    out
}

/// Vérifie les échéances. À appeler au réveil du planificateur et au lancement.
pub fn evaluate(chain: &mut Chain, now: Ts) -> Vec<Effect> {
    let mut out = Vec::new();
    loop {
        if chain.state != ChainState::EnCours {
            break;
        }
        let Some(i) = chain
            .steps
            .iter()
            .position(|s| s.state == StepState::Active)
        else {
            break;
        };
        let due = chain.steps[i].due_at.unwrap_or(i64::MAX);
        if now <= due {
            break;
        }
        if chain.auto_validate {
            chain.steps[i].state = StepState::AutoValidee;
            out.push(Effect::StepAutoValidated { step: i });
            advance(chain, i, due, &mut out);
        } else {
            chain.steps[i].state = StepState::Manquee;
            chain.state = ChainState::Interrompue;
            out.push(Effect::StepMissed { step: i });
            out.push(Effect::ChainInterrupted);
            break;
        }
    }
    out
}

/// Relancer : reprend à l'étape manquée avec un nouveau délai.
pub fn retry(chain: &mut Chain, now: Ts, extra_seconds: i64) {
    if chain.state != ChainState::Interrompue {
        return;
    }
    if let Some(i) = chain
        .steps
        .iter()
        .position(|s| s.state == StepState::Manquee)
    {
        chain.steps[i].state = StepState::Active;
        chain.steps[i].due_at = Some(now + extra_seconds);
        chain.state = ChainState::EnCours;
    }
}

/// Recommencer entièrement la suite.
pub fn restart(chain: &mut Chain, now: Ts) {
    for s in chain.steps.iter_mut() {
        s.state = StepState::EnAttente;
        s.due_at = None;
    }
    chain.state = ChainState::NonDemarree;
    start(chain, now);
}

/// Valider manuellement l'étape manquée (statut AutoValidee) et continuer.
pub fn validate_missed(chain: &mut Chain, now: Ts) -> Vec<Effect> {
    let mut out = Vec::new();
    if chain.state != ChainState::Interrompue {
        return out;
    }
    if let Some(i) = chain
        .steps
        .iter()
        .position(|s| s.state == StepState::Manquee)
    {
        chain.steps[i].state = StepState::AutoValidee;
        out.push(Effect::StepAutoValidated { step: i });
        advance(chain, i, now, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use plano_core::Step;

    fn sample(auto: bool) -> Chain {
        Chain::new(
            "Vidéo",
            vec![
                Step::new("Écrire", Deadline::AfterPrevious(100)),
                Step::new("Filmer", Deadline::AfterPrevious(100)),
                Step::new("Monter", Deadline::AfterPrevious(100)),
            ],
            auto,
        )
    }

    #[test]
    fn rien_avant_l_echeance() {
        let mut c = sample(false);
        start(&mut c, 0);
        assert!(evaluate(&mut c, 50).is_empty());
        assert_eq!(c.state, ChainState::EnCours);
    }

    #[test]
    fn etape_manquee_interrompt_puis_relance() {
        let mut c = sample(false);
        start(&mut c, 0);
        let fx = evaluate(&mut c, 101);
        assert_eq!(
            fx,
            vec![Effect::StepMissed { step: 0 }, Effect::ChainInterrupted]
        );
        assert_eq!(c.state, ChainState::Interrompue);
        assert_eq!(c.steps[1].state, StepState::EnAttente);

        retry(&mut c, 101, 100);
        assert_eq!(c.state, ChainState::EnCours);
        assert_eq!(c.steps[0].due_at, Some(201));
        complete_current(&mut c, 150);
        assert_eq!(c.steps[1].state, StepState::Active);
        assert_eq!(c.steps[1].due_at, Some(250));
    }

    #[test]
    fn recommencer_remet_tout_a_zero() {
        let mut c = sample(false);
        start(&mut c, 0);
        evaluate(&mut c, 101);
        restart(&mut c, 200);
        assert_eq!(c.state, ChainState::EnCours);
        assert_eq!(c.steps[0].due_at, Some(300));
        assert_eq!(c.steps[1].state, StepState::EnAttente);
    }

    #[test]
    fn auto_validation_enchaine_et_termine() {
        let mut c = sample(true);
        start(&mut c, 0);
        let fx = evaluate(&mut c, 350);
        assert_eq!(c.state, ChainState::Terminee);
        assert_eq!(fx.last(), Some(&Effect::ChainCompleted));
        assert!(c.steps.iter().all(|s| s.state == StepState::AutoValidee));
    }

    #[test]
    fn validation_manuelle_de_l_etape_manquee() {
        let mut c = sample(false);
        start(&mut c, 0);
        evaluate(&mut c, 101);
        validate_missed(&mut c, 101);
        assert_eq!(c.steps[0].state, StepState::AutoValidee);
        assert_eq!(c.steps[1].state, StepState::Active);
        assert_eq!(c.state, ChainState::EnCours);
    }
}
