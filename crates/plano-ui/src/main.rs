//! plano-ui : prototype d'interface (iced).
//! Objectif : mesurer taille, mémoire et fluidité sur une machine ancienne
//! avant de figer le choix de la bibliothèque d'interface.

use iced::widget::{button, column, container, row, scrollable, text, text_input, Column, Row};
use iced::{Element, Length, Theme};
use plano_core::{Id, Item, ItemStatus};
use plano_engine::{Clock, SystemClock};
use plano_store::Store;

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title(|_: &App| String::from("Plano — prototype"))
        .theme(App::theme)
        .centered()
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    SelectPlan(Id),
    TitleChanged(String),
    AddTask,
    Move(Id, ItemStatus),
    NextTheme,
    Reload,
}

struct App {
    store: Option<Store>,
    error: Option<String>,
    plans: Vec<(Id, String)>,
    selected: Option<Id>,
    items: Vec<Item>,
    new_title: String,
    theme_index: usize,
}

impl App {
    fn boot() -> Self {
        let path = std::env::var("PLANO_DB").unwrap_or_else(|_| "plano.db".to_string());
        let mut app = App {
            store: None,
            error: None,
            plans: Vec::new(),
            selected: None,
            items: Vec::new(),
            new_title: String::new(),
            theme_index: 0,
        };
        match Store::open(&path) {
            Ok(store) => app.store = Some(store),
            Err(e) => app.error = Some(format!("Impossible d'ouvrir {path} : {e}")),
        }
        app.refresh();
        app
    }

    fn theme(&self) -> Theme {
        match self.theme_index % 5 {
            0 => Theme::Light,
            1 => Theme::Dark,
            2 => Theme::Nord,
            3 => Theme::SolarizedLight,
            _ => Theme::GruvboxDark,
        }
    }

    /// Recharge les plans et les tâches du plan sélectionné depuis le coffre.
    fn refresh(&mut self) {
        let Some(store) = &self.store else {
            return;
        };
        let current = self.selected;
        let result = store.list_plans().and_then(|plans| {
            let selected = current
                .filter(|s| plans.iter().any(|(id, _)| id == s))
                .or_else(|| plans.first().map(|(id, _)| *id));
            let items = match selected {
                Some(plan) => store.list_items(plan)?,
                None => Vec::new(),
            };
            Ok((plans, selected, items))
        });
        match result {
            Ok((plans, selected, items)) => {
                self.plans = plans;
                self.selected = selected;
                self.items = items;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::SelectPlan(id) => {
                self.selected = Some(id);
                self.refresh();
            }
            Message::TitleChanged(title) => self.new_title = title,
            Message::AddTask => {
                let title = self.new_title.trim().to_string();
                if title.is_empty() {
                    return;
                }
                if let (Some(store), Some(plan)) = (&self.store, self.selected) {
                    match store.add_item(plan, &title, SystemClock.now()) {
                        Ok(_) => self.new_title.clear(),
                        Err(e) => self.error = Some(e.to_string()),
                    }
                }
                self.refresh();
            }
            Message::Move(id, status) => {
                if let Some(store) = &self.store {
                    if let Err(e) = store.set_status(id, status, SystemClock.now()) {
                        self.error = Some(e.to_string());
                    }
                }
                self.refresh();
            }
            Message::NextTheme => self.theme_index += 1,
            Message::Reload => self.refresh(),
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let header = row![
            text("Plano").size(28),
            button("Changer de thème").on_press(Message::NextTheme),
            button("Actualiser").on_press(Message::Reload),
            text(format!("Thème : {}", self.theme())),
        ]
        .spacing(16);

        let tabs = self
            .plans
            .iter()
            .fold(Row::new().spacing(8), |tabs, (id, title)| {
                let tab = button(text(title.as_str())).on_press(Message::SelectPlan(*id));
                tabs.push(if Some(*id) == self.selected {
                    tab.style(button::primary)
                } else {
                    tab.style(button::secondary)
                })
            });

        let add = row![
            text_input("Nouvelle tâche…", &self.new_title)
                .on_input(Message::TitleChanged)
                .on_submit(Message::AddTask),
            button("Ajouter").on_press(Message::AddTask),
        ]
        .spacing(8);

        let body: Element<'_, Message> = if self.plans.is_empty() {
            text("Aucun plan. Créez-en un : plano-cli plan add \"Sport\", puis cliquez sur Actualiser.")
                .into()
        } else {
            row![
                self.board_column(
                    "À faire",
                    ItemStatus::APlanifier,
                    None,
                    Some(ItemStatus::EnCours)
                ),
                self.board_column(
                    "En cours",
                    ItemStatus::EnCours,
                    Some(ItemStatus::APlanifier),
                    Some(ItemStatus::Termine)
                ),
                self.board_column("Terminé", ItemStatus::Termine, Some(ItemStatus::EnCours), None),
            ]
            .spacing(12)
            .height(Length::Fill)
            .into()
        };

        let mut content = column![header, tabs, add].spacing(12);
        if let Some(error) = &self.error {
            content = content.push(text(error.as_str()));
        }
        content = content.push(body);
        container(content).padding(16).into()
    }

    fn board_column<'a>(
        &'a self,
        title: &'a str,
        status: ItemStatus,
        left: Option<ItemStatus>,
        right: Option<ItemStatus>,
    ) -> Element<'a, Message> {
        let mut count = 0;
        let cards = self
            .items
            .iter()
            .filter(|item| item.status == status)
            .fold(Column::new().spacing(8), |cards, item| {
                count += 1;
                let mut controls = Row::new().spacing(6);
                if let Some(target) = left {
                    controls = controls.push(button("<").on_press(Message::Move(item.id, target)));
                }
                if let Some(target) = right {
                    controls = controls.push(button(">").on_press(Message::Move(item.id, target)));
                }
                cards.push(
                    container(column![text(item.title.as_str()), controls].spacing(6))
                        .padding(8)
                        .width(Length::Fill)
                        .style(container::bordered_box),
                )
            });

        container(
            column![text(format!("{title} ({count})")).size(20), scrollable(cards)].spacing(10),
        )
        .padding(10)
        .width(Length::FillPortion(1))
        .style(container::rounded_box)
        .into()
    }
}
