//! `UiWriter` — a UI declarativa dirigida de Rust.
//!
//! O `viber.ui.*` só existe dentro do VM Luau. Um jogo escrito em Rust (o
//! crate Vael) usa este recurso para os mesmos setters: cada chamada enfileira
//! um [`UiCommand`] na MESMA fila que os closures Lua enchem, e o
//! `apply_ui_commands` aplica-o em `UiSet::Script`. O writer nunca constrói
//! nós nem toca no `World` — só escreve na fila.
//!
//! ```ignore
//! fn hp_system(hud: Res<UiWriter>, health: Res<Health>) {
//!     let fraction = health.fraction();
//!     hud.set_value("hp-bar", fraction);
//!     hud.toggle_class("hp-orb", "danger", fraction < 0.3);
//! }
//! ```

use bevy::prelude::*;

use super::list::ListRow;
use super::script::{UiCommand, UiCommandQueue};

/// Handle de escrita na fila de comandos da UI declarativa.
///
/// Clone partilha a fila (o `Arc` interno), tal como os closures Lua.
#[derive(Clone, Resource)]
pub struct UiWriter {
    queue: UiCommandQueue,
}

impl From<UiCommandQueue> for UiWriter {
    fn from(queue: UiCommandQueue) -> Self {
        Self { queue }
    }
}

impl UiWriter {
    /// Substitui o texto de um elemento (ou o valor de um `<UiInput>`).
    pub fn set_text(&self, id: impl Into<String>, text: impl Into<String>) {
        self.queue.push(UiCommand::SetText {
            id: id.into(),
            text: text.into(),
        });
    }

    /// Fração de uma `<UiBar>` / `<UiCooldown>` / `<UiSlider>`.
    pub fn set_value(&self, id: impl Into<String>, value: f32) {
        self.queue.push(UiCommand::SetValue {
            id: id.into(),
            value,
        });
    }

    /// Mostra ou esconde um elemento (fade incluído, como no Lua).
    pub fn set_visible(&self, id: impl Into<String>, visible: bool) {
        self.queue.push(UiCommand::SetVisible {
            id: id.into(),
            visible,
        });
    }

    /// Liga/desliga o estado `disabled` (styling e interação).
    pub fn set_disabled(&self, id: impl Into<String>, disabled: bool) {
        self.queue.push(UiCommand::SetDisabled {
            id: id.into(),
            disabled,
        });
    }

    /// Acrescenta uma classe CSS.
    pub fn add_class(&self, id: impl Into<String>, class: impl Into<String>) {
        self.queue.push(UiCommand::AddClass {
            id: id.into(),
            class: class.into(),
        });
    }

    /// Remove uma classe CSS.
    pub fn remove_class(&self, id: impl Into<String>, class: impl Into<String>) {
        self.queue.push(UiCommand::RemoveClass {
            id: id.into(),
            class: class.into(),
        });
    }

    /// Add/remove numa só chamada — o par do `viber.ui.toggle_class`.
    pub fn toggle_class(&self, id: impl Into<String>, class: impl Into<String>, on: bool) {
        let (id, class) = (id.into(), class.into());
        self.queue.push(if on {
            UiCommand::AddClass { id, class }
        } else {
            UiCommand::RemoveClass { id, class }
        });
    }

    /// Marca/desmarca um `<UiCheck>`.
    pub fn set_checked(&self, id: impl Into<String>, checked: bool) {
        self.queue.push(UiCommand::SetChecked {
            id: id.into(),
            checked,
        });
    }

    /// Abre/fecha um `<UiModal>`.
    pub fn open(&self, id: impl Into<String>, open: bool) {
        self.queue.push(UiCommand::SetModal {
            id: id.into(),
            open,
        });
    }

    /// Seleciona um separador dentro de um grupo.
    pub fn select_tab(&self, group: impl Into<String>, tab: impl Into<String>) {
        self.queue.push(UiCommand::SelectTab {
            group: group.into(),
            tab: tab.into(),
        });
    }

    /// Dá o teclado a um `<UiInput>`.
    pub fn focus(&self, id: impl Into<String>) {
        self.queue.push(UiCommand::Focus { id: id.into() });
    }

    /// Substitui a animação de motion (`"none"`/`""` remove).
    pub fn set_anim(&self, id: impl Into<String>, spec: impl Into<String>) {
        self.queue.push(UiCommand::SetAnim {
            id: id.into(),
            spec: spec.into(),
        });
    }

    /// Alimenta uma fonte de `<UiList>` a partir de Rust.
    pub fn set_list(&self, name: impl Into<String>, rows: Vec<ListRow>) {
        self.queue.push(UiCommand::SetList {
            name: name.into(),
            rows,
        });
    }

    /// Devolve a fonte da lista à engine (o par do `viber.ui.unlist`).
    pub fn unlist(&self, name: impl Into<String>) {
        self.queue.push(UiCommand::Unlist { name: name.into() });
    }

    /// Remove o elemento e a subárvore da UI em runtime.
    pub fn destroy(&self, id: impl Into<String>) {
        self.queue.push(UiCommand::Destroy { id: id.into() });
    }

    /// Escala global da UI — o apply limita a 0.5..3.0, como no Lua.
    pub fn scale(&self, value: f32) {
        self.queue.push(UiCommand::SetScale(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fila + writer ligados ao mesmo `Arc` — o que o plugin faz.
    fn writer() -> (UiCommandQueue, UiWriter) {
        let queue = UiCommandQueue::default();
        (queue.clone(), UiWriter::from(queue))
    }

    #[test]
    fn test_every_method_queues_its_command() {
        let (queue, writer) = writer();
        writer.set_text("hp", "42/100");
        writer.set_value("hp-bar", 0.42);
        writer.set_visible("hp", false);
        writer.set_disabled("btn-save", true);
        writer.add_class("orb", "danger");
        writer.remove_class("orb", "danger");
        writer.toggle_class("orb", "danger", true);
        writer.toggle_class("orb", "danger", false);
        writer.set_checked("opt", true);
        writer.open("modal-save", true);
        writer.select_tab("tabs", "gear");
        writer.focus("name");
        writer.set_anim("orb", "pulse 1s");
        let mut row = ListRow::new();
        row.insert("name".into(), "Sword".into());
        writer.set_list("bag", vec![row]);
        writer.unlist("bag");
        writer.destroy("toast");
        writer.scale(1.5);

        let queued = queue.drain();
        assert_eq!(queued.len(), 17, "um comando por chamada");
        assert!(
            matches!(&queued[0], UiCommand::SetText { id, text } if id == "hp" && text == "42/100")
        );
        assert!(
            matches!(&queued[1], UiCommand::SetValue { id, value } if id == "hp-bar" && (*value - 0.42).abs() < 1e-6)
        );
        assert!(
            matches!(&queued[2], UiCommand::SetVisible { id, visible } if id == "hp" && !*visible)
        );
        assert!(
            matches!(&queued[3], UiCommand::SetDisabled { id, disabled } if id == "btn-save" && *disabled)
        );
        assert!(
            matches!(&queued[4], UiCommand::AddClass { id, class } if id == "orb" && class == "danger")
        );
        assert!(
            matches!(&queued[5], UiCommand::RemoveClass { id, class } if id == "orb" && class == "danger")
        );
        assert!(matches!(&queued[6], UiCommand::AddClass { class, .. } if class == "danger"));
        assert!(matches!(&queued[7], UiCommand::RemoveClass { class, .. } if class == "danger"));
        assert!(
            matches!(&queued[8], UiCommand::SetChecked { id, checked } if id == "opt" && *checked)
        );
        assert!(
            matches!(&queued[9], UiCommand::SetModal { id, open } if id == "modal-save" && *open)
        );
        assert!(
            matches!(&queued[10], UiCommand::SelectTab { group, tab } if group == "tabs" && tab == "gear")
        );
        assert!(matches!(&queued[11], UiCommand::Focus { id } if id == "name"));
        assert!(
            matches!(&queued[12], UiCommand::SetAnim { id, spec } if id == "orb" && spec == "pulse 1s")
        );
        assert!(matches!(
            &queued[13],
            UiCommand::SetList { name, rows }
                if name == "bag" && rows.len() == 1 && rows[0].get("name").is_some_and(|v| v == "Sword")
        ));
        assert!(matches!(&queued[14], UiCommand::Unlist { name } if name == "bag"));
        assert!(matches!(&queued[15], UiCommand::Destroy { id } if id == "toast"));
        assert!(matches!(&queued[16], UiCommand::SetScale(value) if (*value - 1.5).abs() < 1e-6));
    }

    #[test]
    fn test_clone_shares_the_same_queue() {
        let (queue, writer) = writer();
        let clone = writer.clone();
        clone.set_text("a", "b");
        // A fila original vê o comando do clone (mesmo `Arc`).
        assert_eq!(queue.drain().len(), 1);
    }

    #[test]
    fn test_writer_sees_commands_queued_directly() {
        let (queue, writer) = writer();
        queue.push(UiCommand::SetText {
            id: "a".into(),
            text: "x".into(),
        });
        writer.set_visible("a", true);
        let queued = queue.drain();
        assert_eq!(queued.len(), 2);
        assert!(matches!(&queued[0], UiCommand::SetText { id, .. } if id == "a"));
        assert!(matches!(&queued[1], UiCommand::SetVisible { id, .. } if id == "a"));
    }
}
