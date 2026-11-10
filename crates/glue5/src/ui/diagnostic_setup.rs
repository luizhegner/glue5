//! Native setup, not a claim that an agent is already connected or watching.
use egui::{RichText, Ui};
use super::{theme, widgets};

#[derive(Default)]
pub struct Setup<'a> {
    pub enabled: bool,
    pub game_running: bool,
    pub mods_running: bool,
    pub mods_enabled: bool,
    pub studio_enabled: bool,
    pub studio_running: bool,
    pub lan: bool,
    pub paired: usize,
    pub address: Option<&'a str>,
    pub code: Option<&'a str>,
}

impl Setup<'_> {
    pub fn state(&self) -> &'static str {
        if !self.enabled { "Desativado" }
        else if !self.mods_enabled { "Ative o suporte a mods" }
        else if self.game_running && !self.mods_running { "Reinicie o jogo com mods ativados" }
        else if !self.studio_enabled { "Ative a conexão com agentes" }
        else if !self.studio_running { "Aguardando serviço de conexão" }
        else if !self.lan { "Conexão restrita a este celular" }
        else if self.paired == 0 { "Aguardando pareamento de um agente" }
        else if !self.game_running { "Inicie o jogo para receber solicitações" }
        else { "Disponível ao voltar ao jogo" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Enable(bool), Mods, Studio, Lan, CopyAddress, CopyCode, CopyConnection }

pub fn show(ui: &mut Ui, setup: &Setup<'_>) -> Vec<Action> {
    let mut actions = Vec::new();
    ui.label(theme::heading("Diagnóstico por IA", 17.0 * theme::density(ui)));
    ui.label(RichText::new(setup.state()).color(theme::MUTED));
    let mut enabled = setup.enabled;
    widgets::row(ui, "Permitir solicitações de IA", "Vale só até fechar o GLUE5. Desativar cancela a sessão. Cada solicitação ainda exige sua autorização.", |ui| widgets::toggle(ui, &mut enabled));
    if enabled != setup.enabled { actions.push(Action::Enable(enabled)); }
    if !setup.enabled { return actions; }

    ui.label("Ativar esta opção não conecta uma IA nem inicia uma depuração automaticamente.");
    if !setup.mods_enabled && button(ui, "Suporte de conexão", "Usa o serviço existente de mods; não instala scripts.", "ATIVAR MODS") {
        actions.push(Action::Mods);
    }
    if setup.mods_enabled && !setup.studio_enabled && button(ui, "Conexão com agentes", "Ativa o serviço GLUE5 Studio usado pela API.", "ATIVAR") {
        actions.push(Action::Studio);
    }
    if setup.studio_running && !setup.lan && button(ui, "Acesso pelo computador", "Permite pareamento na rede local. Use somente uma rede de confiança.", "PERMITIR NA REDE") {
        actions.push(Action::Lan);
    }
    if setup.studio_running {
        if let Some(address) = setup.address {
            if button(ui, "Endereço para o agente (MCP)", address, "COPIAR") { actions.push(Action::CopyAddress); }
        }
        if setup.code.is_some() && button(ui, "Código de pareamento", "É uma credencial de acesso ao Studio. Compartilhe só com o agente de confiança; não publique em logs.", "COPIAR CÓDIGO") {
            actions.push(Action::CopyCode);
        }
        if setup.lan && button(ui, "Criar acesso para um agente", "Cria um cliente Studio e copia a configuração MCP autenticada. Dá acesso a mods e scripts, não só diagnóstico. Cole apenas no agente de confiança. Revogue em Mods > Studio > Paired.", "CRIAR E COPIAR") {
            actions.push(Action::CopyConnection);
        }
        if ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("diagnostic-connection-copied")).unwrap_or(false)) {
            ui.label("Acesso criado e configuração copiada. Cole no agente de confiança; não precisa criar outro acesso para o mesmo agente.");
        }
        ui.label(format!("Clientes Studio pareados: {}. Pareado não significa conectado ou controlando.", setup.paired));
    }
    ui.separator();
    ui.label("1. Use Criar e copiar para obter uma conexão MCP autenticada e configure o agente. O código avulso, sozinho, não autoriza diagnóstico.");
    ui.label("2. Salve seu progresso e volte ao jogo. O agente solicita controle pela ferramenta diagnostic_control.");
    ui.label("3. Autorize no celular. O aviso e Parar IA aparecem durante a sessão; Voltar também interrompe.");
    ui.label(RichText::new("Disponível: controle remoto autorizado e logs do Studio. Ainda não inclui observação de imagem, reprodução automática de bugs ou proteção dos saves. Não inicia captura Shizuku.").small().color(theme::MUTED));
    actions
}

fn button(ui: &mut Ui, title: &str, help: &str, label: &str) -> bool {
    widgets::row(ui, title, help, |ui| widgets::action_value(ui, label)).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_never_calls_a_paired_client_connected_or_active() {
        let mut s = Setup::default();
        assert_eq!(s.state(), "Desativado");
        s.enabled = true;
        assert_eq!(s.state(), "Ative o suporte a mods");
        s.mods_enabled = true;
        s.studio_enabled = true;
        s.studio_running = true;
        s.lan = true;
        assert_eq!(s.state(), "Aguardando pareamento de um agente");
        s.paired = 1;
        assert_eq!(s.state(), "Inicie o jogo para receber solicitações");
        s.game_running = true;
        assert_eq!(s.state(), "Reinicie o jogo com mods ativados");
        s.mods_running = true;
        assert_eq!(s.state(), "Disponível ao voltar ao jogo");
    }
}
