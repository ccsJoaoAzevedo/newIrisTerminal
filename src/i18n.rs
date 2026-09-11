//! Interface language.
//!
//! One table, keyed by the English text as it appears in the source. Wrapping a
//! literal in [`tr`] is the whole of the work at a call site, and an English
//! string with no entry comes out in English rather than as a missing key — a
//! new label is untranslated, never blank.
//!
//! The Portuguese follows the vocabulary of the native IrisTerm where the two
//! name the same thing ("Núm. de Linhas de Rolagem", "Salvar", "Cancelar"), so
//! that anyone moving between the two terminals is reading the same words.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Language the interface is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    #[serde(rename = "en")]
    En,
    #[serde(rename = "pt-br")]
    PtBr,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::PtBr];

    /// Shown in the picker, in the language itself: someone looking for
    /// Portuguese is looking for the word "Português".
    pub fn label(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::PtBr => "Português (Brasil)",
        }
    }
}

/// The language every [`tr`] answers in.
///
/// A global rather than a parameter: the alternative is threading a language
/// through every `fn(ui, ...)` in the interface, and there is exactly one
/// interface. Stored as a `u8` so it needs no lock.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(lang: Lang) {
    CURRENT.store(
        match lang {
            Lang::En => 0,
            Lang::PtBr => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn language() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::PtBr,
        _ => Lang::En,
    }
}

/// The Brazilian Portuguese for one English string, or the English itself.
pub fn tr(text: &'static str) -> &'static str {
    match language() {
        Lang::En => text,
        Lang::PtBr => pt_br().get(text).copied().unwrap_or(text),
    }
}

/// [`tr`] for a string with one `{}` in it, filled in after translation.
///
/// The placeholder travels with the sentence, so a language that needs it in a
/// different position can move it.
pub fn tr1(text: &'static str, arg: &str) -> String {
    tr(text).replacen("{}", arg, 1)
}

/// [`tr1`] for a sentence with two placeholders, filled in left to right.
pub fn tr2(text: &'static str, first: &str, second: &str) -> String {
    tr(text).replacen("{}", first, 1).replacen("{}", second, 1)
}

fn pt_br() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| PT_BR.iter().copied().collect())
}

/// English source text, and its Brazilian Portuguese.
#[rustfmt::skip]
const PT_BR: &[(&str, &str)] = &[
    // Menu bar, tabs, and the window itself.
    ("Macros", "Macros"),
    ("Export", "Exportar"),
    ("Settings", "Configurações"),
    ("Themes", "Temas"),
    ("Preview", "Prévia"),

    // Updates.
    ("Updates", "Atualizações"),
    ("Update available", "Atualização disponível"),
    ("Version {} is available. This one is {}.", "A versão {} está disponível. Esta é a {}."),
    ("Download", "Baixar"),
    ("Downloading... {} of {}", "Baixando... {} de {}"),
    ("Downloading... {}", "Baixando... {}"),
    ("Could not download the update: {}", "Não foi possível baixar a atualização: {}"),
    ("Or fetch it yourself:", "Ou baixe você mesmo:"),
    ("open the download", "abrir o download"),
    ("Save it next to the running program, then replace the program with it.",
     "Salve ao lado do programa em execução, depois substitua o programa por ele."),
    ("Proxy user", "Usuário do proxy"),
    ("Password", "Senha"),
    ("Only if the proxy asks for credentials. Leave empty otherwise.",
     "Somente se o proxy pedir credenciais. Deixe vazio caso contrário."),
    ("Kept in the operating system's credential store, never in settings.toml.",
     "Guardada no cofre de credenciais do sistema operacional, nunca em settings.toml."),
    ("Basic authentication only. A proxy that insists on NTLM cannot be reached this way; download the release from the browser instead.",
     "Somente autenticação Basic. Um proxy que exige NTLM não pode ser acessado assim; baixe a versão pelo navegador."),
    ("stored", "guardada"),
    ("none", "nenhuma"),
    ("Proxy password saved.", "Senha do proxy salva."),
    ("Proxy password forgotten.", "Senha do proxy esquecida."),
    ("Could not save the proxy password: {}",
     "Não foi possível salvar a senha do proxy: {}"),
    ("Restart and update", "Reiniciar e atualizar"),
    ("Puts the new version in place and starts it. A session still connected is asked about first.",
     "Coloca a nova versão no lugar e a inicia. Uma sessão ainda conectada é perguntada antes."),
    ("Later", "Depois"),
    ("Check for a new version at startup", "Procurar uma nova versão ao iniciar"),
    ("One request to GitHub through the machine's own proxy. Nothing is downloaded or replaced without being asked.",
     "Uma requisição ao GitHub pelo proxy da própria máquina. Nada é baixado ou substituído sem ser solicitado."),
    ("This build is version {}.", "Esta compilação é a versão {}."),
    ("Check now", "Verificar agora"),
    ("Going through the system proxy at {}.", "Passando pelo proxy do sistema em {}."),
    ("No system proxy configured; connecting directly.",
     "Nenhum proxy de sistema configurado; conectando diretamente."),
    ("This is the newest version ({}).", "Esta é a versão mais recente ({})."),
    ("Could not check for updates: {}", "Não foi possível verificar atualizações: {}"),
    ("Could not apply the update: {}", "Não foi possível aplicar a atualização: {}"),
    ("Open a session (Ctrl+T)", "Abrir uma sessão (Ctrl+T)"),
    ("IRIS servers", "Servidores IRIS"),
    ("No servers, profiles or instances found.", "Nenhum servidor, perfil ou instância encontrado."),
    ("Double-click to rename.", "Clique duplo para renomear."),
    ("Session ended.", "Sessão encerrada."),
    ("Reconnect", "Reconectar"),
    ("Close", "Fechar"),
    ("Minimize", "Minimizar"),
    ("Maximize", "Maximizar"),
    ("Restore", "Restaurar"),
    ("Rename", "Renomear"),
    ("Rename...", "Renomear..."),
    ("Rename tab", "Renomear aba"),
    ("Empty goes back to default.", "Vazio volta ao padrão."),
    ("Split to right", "Dividir à direita"),
    ("Split to bottom", "Dividir abaixo"),
    ("Remove split", "Remover divisão"),
    ("Opens a second session in this tab, beside this one. Click into a pane to type in it.",
     "Abre uma segunda sessão nesta aba, ao lado desta. Clique em um painel para digitar nele."),
    ("Gives the second session a tab of its own. Nothing is closed.",
     "Dá à segunda sessão uma aba própria. Nada é fechado."),
    ("Closes both sessions in this tab.", "Fecha as duas sessões desta aba."),
    ("Closes this session.", "Fecha esta sessão."),
    ("Close pane", "Fechar painel"),
    ("Closes this session. The other pane stays, in a tab of its own.",
     "Fecha esta sessão. O outro painel permanece, em uma aba própria."),
    ("Close anyway", "Fechar de qualquer forma"),
    ("Keep working", "Continuar trabalhando"),
    ("Closing sends HALT to each of them.", "Fechar envia HALT para cada uma delas."),
    ("dismiss", "descartar"),
    ("Copy", "Copiar"),
    ("Paste", "Colar"),
    ("Copy and paste", "Copiar e colar"),
    ("Puts the selection on the clipboard and types it at the prompt.",
     "Coloca a seleção na área de transferência e a digita no prompt."),
    ("Select all", "Selecionar tudo"),
    ("Analyze with Claude", "Analisar com o Claude"),
    ("This pane", "Este painel"),
    ("Both panes", "Ambos os painéis"),
    ("All output", "Toda a saída"),
    ("Last 10 commands", "Últimos 10 comandos"),
    ("Last 5 commands", "Últimos 5 comandos"),
    ("Could not prepare the output: {}", "Não foi possível preparar a saída: {}"),
    ("Claude is opening with this output in context ({}). Ask it whatever you like.",
     "O Claude está abrindo com esta saída no contexto ({}). Pergunte o que quiser."),
    ("Clear terminal and scrollback", "Limpar terminal e histórico de rolagem"),
    ("Ctrl+Delete. Unlike a clear-screen from the session itself, this really does throw the history away. It asks the far side to clear - W # at an idle IRIS prompt, Ctrl+L in a shell - so the next prompt goes back to the top; the echo and the old screen are dropped rather than kept.",
     "Ctrl+Delete. Diferente do clear-screen da própria sessão, este realmente descarta o histórico. Ele pede que o outro lado limpe a tela - W # em um prompt inativo do IRIS, Ctrl+L em um shell - para que o próximo prompt volte ao topo; o eco e a tela antiga são descartados em vez de guardados."),

    // Settings: sections and appearance.
    ("Appearance", "Aparência"),
    ("Session", "Sessão"),
    ("Window", "Janela"),
    ("Logging", "Registro em log"),
    ("Profiles", "Perfis"),
    ("Language", "Idioma"),
    ("Theme", "Tema"),
    ("Manage themes...", "Gerenciar temas..."),
    ("Duplicate a built-in theme and change any of its colours, including the ObjectScript ones.",
     "Duplique um tema nativo e altere qualquer uma de suas cores, inclusive as do ObjectScript."),
    ("Open folder", "Abrir pasta"),
    ("Font", "Fonte"),
    ("Font size", "Tamanho da fonte"),
    ("Built-in monospace", "Monoespaçada interna"),
    ("Monospace families only: the terminal is a character grid, so a proportional font would not line up.",
     "Somente famílias monoespaçadas: o terminal é uma grade de caracteres, e uma fonte proporcional não alinharia."),
    ("Cursor", "Cursor"),
    ("Block", "Bloco"),
    ("Bar", "Barra"),
    ("Underscore", "Sublinhado"),
    ("Blink", "Piscar"),
    ("Syntax highlighting", "Realce de sintaxe"),
    ("Colours globals, strings, numbers, commands, macros and class references. A guess about the text on screen; a colour IRIS sets itself always wins.",
     "Colore globais, strings, números, comandos, macros e referências a classes. É uma suposição sobre o texto na tela; uma cor definida pelo próprio IRIS sempre prevalece."),
    ("Show scrollbars", "Mostrar barras de rolagem"),
    ("Solid scrollbars instead of the thin ones that only appear on hover.",
     "Barras de rolagem sólidas em vez das finas que só aparecem ao passar o mouse."),
    ("The built-in themes are read-only; duplicating one in the theme manager gives you a copy to edit. A theme file dropped into the themes folder by hand is picked up at the next start.",
     "Os temas nativos são somente leitura; duplicar um no gerenciador de temas cria uma cópia editável. Um arquivo de tema colocado na pasta de temas à mão é carregado na próxima inicialização."),

    // Settings: session.
    ("Scrollback lines", "Núm. de Linhas de Rolagem"),
    ("Hide status messages after", "Ocultar mensagens de status após"),
    ("Seconds before a message in the footer goes away on its own. 0 leaves it until it is dismissed.",
     "Segundos até uma mensagem no rodapé sumir sozinha. 0 a mantém até ser descartada."),
    ("Terminal size", "Tamanho do terminal"),
    ("Columns", "Colunas"),
    ("Rows", "Linhas"),
    ("The size a window opens at when it is not reopening at the last one. In characters, so it holds the same amount of output at any font size.",
     "O tamanho com que uma janela abre quando não está reabrindo no tamanho anterior. Em caracteres, de modo que comporta a mesma quantidade de saída em qualquer tamanho de fonte."),
    ("Wrap long lines", "Quebrar linhas longas"),
    ("On: a long line continues on the next row, breaking at the window edge. Off: it runs off to the right, reached by scrolling sideways or widening the window.",
     "Ligado: a linha longa continua na linha seguinte, quebrando na borda da janela. Desligado: ela segue para a direita, alcançada rolando na horizontal ou alargando a janela."),
    ("Either way the whole line is kept: the terminal is reported wider than the window, because IRIS cuts a line at the terminal width instead of wrapping it.",
     "De qualquer forma a linha inteira é mantida: o terminal é informado como mais largo que a janela, porque o IRIS corta a linha na largura do terminal em vez de quebrá-la."),
    ("Copy on select", "Copiar ao selecionar"),
    ("Put a selection on the clipboard as soon as the mouse is released, without waiting for Ctrl+C.",
     "Coloca a seleção na área de transferência assim que o mouse é liberado, sem esperar Ctrl+C."),
    ("Find", "Localizar"),
    ("text in the output", "texto na saída"),
    ("Previous match (Shift+Enter)", "Ocorrência anterior (Shift+Enter)"),
    ("Next match (Enter)", "Próxima ocorrência (Enter)"),
    ("Match upper and lower case exactly", "Diferenciar maiúsculas de minúsculas"),
    ("no matches", "nenhuma ocorrência"),
    ("{} of {}", "{} de {}"),
    ("Quotes and brackets wrap the selection",
     "Aspas e parênteses envolvem a seleção"),
    ("On: typing \" ' ( [ or { over selected text on the command line puts the pair around it instead of replacing it, the way an editor does - so selecting a global name and pressing \" quotes it, and the text stays selected to be wrapped again. Off: the character replaces the selection. Only applies to a selection inside the line being typed; one in the scrollback is highlighted text and is never edited.",
     "Ligado: digitar \" ' ( [ ou { sobre um texto selecionado na linha de comando coloca o par ao redor dele em vez de substituí-lo, como faz um editor - assim, selecionar o nome de um global e pressionar \" o coloca entre aspas, e o texto continua selecionado para ser envolvido novamente. Desligado: o caractere substitui a seleção. Vale apenas para uma seleção dentro da linha que está sendo digitada; uma na rolagem é apenas texto destacado e nunca é editada."),
    ("Up and Down recall from anywhere on the line",
     "Setas acima e abaixo recuperam de qualquer ponto da linha"),
    ("On: Up replaces the line with an earlier command wherever the cursor is, the way the native IRIS terminal does. Off: only at the end of the line, so a cursor left in the middle means the line is being edited and the arrows leave it alone.",
     "Ligado: a seta acima substitui a linha por um comando anterior onde estiver o cursor, como faz o terminal IRIS nativo. Desligado: somente no fim da linha, de modo que um cursor deixado no meio significa que a linha está sendo editada e as setas não a alteram."),
    ("Remember commands from earlier sessions", "Lembrar comandos de sessões anteriores"),
    ("Keeps the commands typed at an IRIS prompt in history.txt, so Up reaches back past the sessions open now. Off keeps recall working inside each session and writes nothing to disk. Either way a tab offers back its own commands first and the inherited ones after them, and lines sent by a macro or an IRIS helper are never offered back at all.",
     "Guarda os comandos digitados no prompt do IRIS em history.txt, de modo que a seta acima alcança sessões anteriores às abertas agora. Desligado mantém a recuperação dentro de cada sessão e não grava nada em disco. De todo modo, uma aba oferece primeiro os comandos dela e depois os herdados, e linhas enviadas por macro ou por um utilitário IRIS nunca são oferecidas de volta."),
    ("Show the namespace in the tab name", "Mostrar o namespace no nome da aba"),
    ("Adds the namespace the session is in to the tab's name - CONSISTEM | RDB76-TR. Read off the prompt, so it follows a ZN as it happens; a tab renamed by hand keeps the name it was given.",
     "Acrescenta ao nome da aba o namespace em que a sessão está - CONSISTEM | RDB76-TR. Lido do prompt, então acompanha um ZN na hora; uma aba renomeada à mão mantém o nome que recebeu."),
    ("Show the process id", "Mostrar o ID do processo"),
    ("Puts the session's process id next to the instance name and the window size in the menu bar. A local session only: a remote one runs its process on the far side.",
     "Coloca o ID do processo da sessão ao lado do nome da instância e do tamanho da janela, na barra de menu. Somente sessão local: uma remota executa seu processo do outro lado."),
    ("Open the default profile at startup", "Abrir o perfil padrão ao iniciar"),
    ("Ask before closing with a session still connected", "Perguntar antes de fechar com uma sessão ainda conectada"),

    // Settings: window.
    ("Show the window buttons", "Mostrar os botões da janela"),
    ("Tabs on window title bar", "Abas na barra de título da janela"),
    ("Puts the tabs on the same row as the window buttons, from the new-session button across to the gear. One row instead of two; the session line - instance, PID and size - goes, since the tabs already say which session it is.",
     "Coloca as abas na mesma linha dos botões da janela, do botão de nova sessão até a engrenagem. Uma linha em vez de duas; a linha da sessão - instância, PID e tamanho - sai, já que as abas já dizem qual sessão é."),
    ("Close this tab.", "Fechar esta aba."),
    ("Minimize, maximize and close in the app's own title bar. Off leaves the row to the tabs: the window still drags, double-click still maximizes, and Ctrl+W still closes. The active theme decides how the buttons look and which end they sit at.",
     "Minimizar, maximizar e fechar na barra de título do próprio aplicativo. Desligado deixa a linha para as abas: a janela continua arrastável, o clique duplo continua maximizando e Ctrl+W continua fechando. O tema ativo decide a aparência dos botões e em qual extremidade ficam."),
    ("Save terminal size", "Salvar tamanho do terminal"),
    ("Reopens the window at the size it was last closed at. Off opens it at 100x30 characters, whatever the font size.",
     "Reabre a janela no tamanho em que foi fechada. Desligado abre em 100x30 caracteres, qualquer que seja o tamanho da fonte."),
    ("Save window position", "Salvar posição da janela"),
    ("Reopens the window where it was last closed. Off centres it on the screen.",
     "Reabre a janela onde foi fechada. Desligado a centraliza na tela."),
    ("Takes effect the next time the app starts, and covers this window as well as the main one.",
     "Passa a valer na próxima inicialização do aplicativo, e vale para esta janela além da principal."),

    // Settings: shells.
    ("Shells", "Shells"),
    ("Other command interpreters, offered under Shells in the new-session menu. Every one of them is a .toml file in the folder below - the ones found installed on this machine were written there for you, and can be renamed, re-armed or deleted like any other.",
     "Outros interpretadores de comando, oferecidos em Shells no menu de nova sessão. Cada um deles é um arquivo .toml na pasta abaixo - os encontrados instalados nesta máquina foram escritos lá para você, e podem ser renomeados, reconfigurados ou excluídos como qualquer outro."),
    ("None found.", "Nenhum encontrado."),
    ("found on this machine", "encontrado nesta máquina"),
    ("declared", "declarado"),
    ("Reload", "Recarregar"),
    ("Probe again and re-read the folder.", "Procurar novamente e reler a pasta."),

    // Settings: macros, logging, profiles.
    ("Organization macro file (shared, read-only)", "Arquivo de macros da organização (compartilhado, somente leitura)"),
    ("A UNC share, mapped drive, or local copy. Leave empty for none.",
     "Um compartilhamento UNC, unidade mapeada ou cópia local. Deixe vazio para nenhum."),
    ("Manage macros...", "Gerenciar macros..."),
    ("Make, edit and delete your own macros, and read the organization's. Running them is on the terminal's right-click menu.",
     "Crie, edite e exclua suas próprias macros, e leia as da organização. Executá-las é pelo menu de contexto do terminal."),
    ("Found.", "Encontrado."),
    ("Not reachable right now - personal macros will still load.",
     "Inacessível neste momento - as macros pessoais continuarão carregando."),
    ("Default mode", "Modo padrão"),
    ("Off", "Desligado"),
    ("Clean text", "Texto limpo"),
    ("Raw bytes", "Bytes brutos"),
    ("Keep logs for (days, 0 = forever)", "Manter logs por (dias, 0 = sempre)"),
    ("Logs: {}", "Logs: {}"),
    ("Add profile", "Adicionar perfil"),
    ("Delete this profile", "Excluir este perfil"),
    ("Instance", "Instância"),
    ("Choose...", "Escolher..."),
    ("Namespace", "Namespace"),
    ("Encoding", "Codificação"),
    ("Leave as UTF-8 unless accented characters come out wrong.",
     "Deixe em UTF-8 a menos que os caracteres acentuados saiam errados."),
    ("Default", "Padrão"),
    ("Save", "Salvar"),
    ("Cancel", "Cancelar"),
    ("Reveal", "Revelar"),

    // The macro manager, and the macros submenu of the right-click menu.
    ("Filter", "Filtrar"),
    ("No macros defined.", "Nenhuma macro definida."),
    ("New macro", "Nova macro"),
    ("New group", "Novo grupo"),
    ("Group name", "Nome do grupo"),
    ("Confirm", "Confirmar"),
    ("Rename group...", "Renomear grupo..."),
    ("New macro in this group", "Nova macro neste grupo"),
    ("A new macro in {}.", "Uma nova macro em {}."),
    ("A new macro in this group.", "Uma nova macro neste grupo."),
    ("Click a group first: a macro is always in one.",
     "Clique em um grupo primeiro: toda macro está em um."),
    ("Only your own groups; the organization's file is never written.",
     "Somente os seus próprios grupos; o arquivo da organização nunca é gravado."),
    ("Groups are how the right-click menu is arranged.",
     "Os grupos são como o menu de contexto é organizado."),
    ("Pick a macro on the left, or make a new one.",
     "Escolha uma macro à esquerda, ou crie uma nova."),
    ("A copy of {} in your own macros.", "Uma cópia de {} nas suas próprias macros."),
    ("{} copy", "{} cópia"),
    ("Only your own macros; the organization's file is never written.",
     "Somente as suas próprias macros; o arquivo da organização nunca é gravado."),
    ("Delete {}?", "Excluir {}?"),
    ("Writes your personal macro file.", "Grava o seu arquivo pessoal de macros."),
    ("Revert", "Reverter"),
    ("Back to what is in the file.", "Volta ao que está no arquivo."),
    ("Picking another macro saves this one too.",
     "Escolher outra macro também salva esta."),
    ("Sends it to the active session, asking for parameters and confirmation exactly as the right-click menu does.",
     "Envia para a sessão ativa, pedindo parâmetros e confirmação exatamente como o menu de contexto faz."),
    ("Each one becomes a field in the right-click menu, filled in before the macro is sent.",
     "Cada um se torna um campo no menu de contexto, preenchido antes de a macro ser enviada."),
    ("Reset", "Restaurar"),
    ("Shortcut for the manager", "Atalho para o gerenciador"),
    ("Back to the fields this helper starts with.",
     "Volta aos campos com que este auxiliar começa."),
    ("Shortcut", "Atalho"),
    ("Detect", "Detectar"),
    ("Press the keys...", "Pressione as teclas..."),
    ("Press the combination and it is filled in here. Esc cancels, Backspace clears it.",
     "Pressione a combinação e ela é preenchida aqui. Esc cancela, Backspace limpa."),
    ("A modifier is required: Ctrl, Alt, or both, with or without Shift.",
     "É necessário um modificador: Ctrl, Alt ou ambos, com ou sem Shift."),
    ("not a shortcut this app understands", "não é um atalho que este aplicativo entende"),
    ("Not understood, so it will not fire. Needs a modifier, like Ctrl+Shift+G.",
     "Não reconhecido, então não será disparado. Precisa de um modificador, como Ctrl+Shift+G."),
    ("Parameters", "Parâmetros"),
    ("Add parameter", "Adicionar parâmetro"),
    ("Name", "Nome"),
    ("Description", "Descrição"),
    ("Name used as {{name}} in the body", "Nome usado como {{name}} no corpo"),
    ("Prompt shown when running", "Rótulo exibido ao executar"),
    ("Default value", "Valor padrão"),
    ("Body - one command per line, {{param}} is substituted",
     "Corpo - um comando por linha, {{param}} é substituído"),
    ("Confirm before sending (use for anything that writes)",
     "Confirmar antes de enviar (use para qualquer coisa que grave)"),
    ("Hide command", "Ocultar comando"),
    ("For a body that carries a password. Keeps it out of the menus; IRIS still echoes what it is sent.",
     "Para um corpo que carrega uma senha. Mantém-na fora dos menus; o IRIS ainda ecoa o que recebe."),
    ("Edit", "Editar"),
    ("Delete", "Excluir"),
    ("Provided by the organization; read-only here.", "Fornecida pela organização; somente leitura aqui."),
    ("Provided by the organization; read-only here. Duplicate it to make changes.",
     "Fornecida pela organização; somente leitura aqui. Duplique-a para fazer alterações."),
    ("org", "org"),
    ("Run", "Executar"),
    ("Will send:", "Vai enviar:"),
    ("Hidden; this macro carries a secret.", "Oculto; esta macro carrega um segredo."),
    ("This macro is marked as modifying data. RDB* databases are shared with the team.",
     "Esta macro está marcada como alteradora de dados. Bases RDB* são compartilhadas com a equipe."),

    // IRIS helpers.
    ("IRIS utilities", "Utilitários IRIS"),
    ("Compile classes", "Compilar classes"),
    ("Compile routines", "Compilar rotinas"),
    ("Generate interface", "Gerar interface"),
    ("Package", "Pacote"),
    ("Flag", "Flag"),
    ("Routine/Group", "Rotina/Grupo"),
    ("Compile classes: {} (flag {})", "Compilar classes: {} (flag {})"),
    ("Compile routines: {}", "Compilar rotinas: {}"),
    ("Generate interface: {}", "Gerar interface: {}"),

    // Encodings. The name of a codepage is not language; what the label says
    // about it is.
    ("CP850 (DOS Western)", "CP850 (DOS Ocidental)"),
    ("A local session speaks UTF-8.", "Uma sessão local fala UTF-8."),
    ("Class", "Classe"),

    // Export.
    ("Screen", "Tela"),
    ("Everything", "Tudo"),
    ("Screen as text", "Tela como texto"),
    ("Everything as text", "Tudo como texto"),
    ("Screen as HTML", "Tela como HTML"),
    ("Everything as HTML", "Tudo como HTML"),
    ("Save to a file", "Salvar em um arquivo"),
    ("Copy to the clipboard", "Copiar para a área de transferência"),

    // Theme manager.
    ("Built-in", "Nativos"),
    ("My themes", "Meus temas"),
    ("None yet - duplicate one.", "Nenhum ainda - duplique um."),
    ("No themes.", "Nenhum tema."),
    ("built-in, read-only", "nativo, somente leitura"),
    ("Duplicate", "Duplicar"),
    ("New from dark", "Novo a partir de um escuro"),
    ("New from light", "Novo a partir de um claro"),
    ("Keep", "Manter"),
    ("Apply", "Aplicar"),
    ("Already in use", "Já está em uso"),
    ("On the left", "À esquerda"),
    ("Shown", "Exibir"),
    ("Whether the title bar has this control at all. Hiding it takes it out of the row; Ctrl+W and Alt+F4 still close the window.",
     "Se a barra de título terá este controle. Ocultá-lo o remove da linha; Ctrl+W e Alt+F4 continuam fechando a janela."),
    ("Back to the colour the button style supplies.",
     "Voltar à cor fornecida pelo estilo do botão."),
    ("Style", "Estilo"),
    ("Stroked", "Traçado"),
    ("Aqua", "Aqua"),
    ("Luna", "Luna"),
    ("Terminal", "Terminal"),
    ("Chrome", "Moldura"),
    ("Base", "Base"),
    ("Window buttons", "Botões da janela"),
    ("ANSI", "ANSI"),
    ("ObjectScript syntax", "Sintaxe ObjectScript"),
    ("Background", "Fundo"),
    ("Foreground", "Primeiro plano"),
    ("Selection", "Seleção"),
    ("Text", "Texto"),
    ("Dark", "Escuro"),
    ("Light", "Claro"),
    ("Glyph", "Símbolo"),
    ("Close hover", "Fechar sob o mouse"),
    ("Settings gear", "Engrenagem de configurações"),
    ("New tab +", "Nova aba +"),
    ("Label", "Rótulo"),
    ("Command", "Comando"),
    ("String", "String"),
    ("Number", "Número"),
    ("Delimiter", "Delimitador"),
    ("Operator", "Operador"),
    ("Preprocessor", "Pré-processador"),
    ("Function", "Função"),
    ("Global", "Global"),
    ("System variable", "Variável de sistema"),
    ("Method", "Método"),
    ("Attribute", "Atributo"),
    ("Member", "Membro"),
    ("Routine", "Rotina"),
    ("Extrinsic", "Extrínseca"),
    ("Duplicate it to change anything - a built-in is the same in every install, which is what makes it something to fall back to.",
     "Duplique-o para alterar qualquer coisa - um tema nativo é igual em toda instalação, e é isso que o torna algo a que recorrer."),
    ("The tab strip, the panels and the dialogs - the frame around the terminal rather than the terminal itself.",
     "A barra de abas, os painéis e as caixas de diálogo - a moldura em torno do terminal, e não o terminal em si."),
    ("Which set of egui widget colours the chrome is built on.",
     "Sobre qual conjunto de cores de widget do egui a moldura é construída."),
    ("The theme's suggestion. The font size in Settings wins over it.",
     "A sugestão do tema. O tamanho da fonte em Configurações prevalece sobre ela."),
    ("Settings -> Window turns the buttons off altogether.",
     "Configurações -> Janela desliga os botões por completo."),
    ("Only your own themes; the built-ins cannot be deleted.",
     "Somente os seus próprios temas; os nativos não podem ser excluídos."),
    ("0-7 normal, 8-15 bright: the sixteen colours IRIS can ask for by number.",
     "0-7 normais, 8-15 brilhantes: as dezesseis cores que o IRIS pode pedir por número."),
    ("Named after the semantic token scopes of the InterSystems VS Code extension, so an editor colour customisation can be copied across field by field.",
     "Nomeadas conforme os escopos de token semântico da extensão InterSystems para VS Code, de modo que uma customização de cores do editor pode ser copiada campo por campo."),
    ("Saved in {}", "Salvo em {}"),
    // Statuses and the odd corners.
    ("Runs after confirming.", "Executa após confirmar."),
    ("Sends this macro to the active session.", "Envia esta macro para a sessão ativa."),
    ("confirms", "confirma"),
    ("1 command hidden", "1 comando oculto"),
    ("{} commands hidden", "{} comandos ocultos"),
    ("The app already uses this for {}; add Shift.", "O aplicativo já usa este atalho para {}; adicione Shift."),
    ("Confirm: {}", "Confirmar: {}"),
    ("Yes, send it", "Sim, enviar"),
    ("Send", "Enviar"),
    ("Profile {}", "Perfil {}"),
    ("Clear selection", "Limpar seleção"),
    ("Close newIrisTerminal?", "Fechar o newIrisTerminal?"),
    ("1 session is still connected.", "1 sessão ainda está conectada."),
    ("{} sessions are still connected.", "{} sessões ainda estão conectadas."),
    ("New session on {} (Ctrl+T).\nRight-click to connect somewhere else.",
     "Nova sessão em {} (Ctrl+T).\nClique com o botão direito para conectar em outro lugar."),
    ("Local session on instance {}", "Sessão local na instância {}"),
    ("Telnet login to {}", "Login Telnet em {}"),
    ("session started ({})", "sessão iniciada ({})"),
    ("Font {} is not installed; using the built-in monospace.",
     "A fonte {} não está instalada; usando a monoespaçada interna."),
    ("Copied to the clipboard.", "Copiado para a área de transferência."),
    ("Could not save settings: {}", "Não foi possível salvar as configurações: {}"),
    ("Could not open {}: {}", "Não foi possível abrir {}: {}"),
    ("Saved personal macros to {}", "Macros pessoais salvas em {}"),
    ("Could not save macros: {}", "Não foi possível salvar as macros: {}"),
    ("Could not save theme: {}", "Não foi possível salvar o tema: {}"),
    ("Could not delete {}: {}", "Não foi possível excluir {}: {}"),
    ("Deleted theme {}.", "Tema {} excluído."),
    ("No active session.", "Nenhuma sessão ativa."),
    ("That session has ended.", "Essa sessão foi encerrada."),
    ("No active session to export.", "Nenhuma sessão ativa para exportar."),
    ("Exported to {}", "Exportado para {}"),
    ("Export failed: {}", "Falha na exportação: {}"),
    ("Delete \"{}\"?", "Excluir \"{}\"?"),
    ("A copy of {} that you can edit.", "Uma cópia de {} que você pode editar."),
    ("Show and apply {}", "Exibir e aplicar {}"),
    ("In use", "Em uso"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A duplicate key would mean one of the two translations is unreachable,
    /// and which one is decided by the order of the table.
    #[test]
    fn no_english_string_appears_twice() {
        let mut seen: Vec<&str> = Vec::new();
        for (en, _) in PT_BR {
            assert!(!seen.contains(en), "{en:?} is in the table twice");
            seen.push(en);
        }
        assert_eq!(pt_br().len(), PT_BR.len());
    }

    /// An empty translation would blank a label out, which is worse than
    /// leaving it in English.
    #[test]
    fn nothing_translates_to_nothing() {
        for (en, pt) in PT_BR {
            assert!(!en.trim().is_empty());
            assert!(!pt.trim().is_empty(), "{en:?} has no translation");
        }
    }

    /// A sentence with a placeholder has to keep it, or the value it stands for
    /// is dropped on the floor.
    #[test]
    fn a_placeholder_survives_translation() {
        for (en, pt) in PT_BR {
            assert_eq!(
                en.matches("{}").count(),
                pt.matches("{}").count(),
                "{en:?} loses or gains a placeholder"
            );
        }
    }

    /// Both languages in one test: the current language is a global, and two
    /// tests setting it would race with each other.
    #[test]
    fn each_language_answers_for_itself() {
        set_language(Lang::En);
        assert_eq!(tr("Settings"), "Settings");
        assert_eq!(tr1("Logs: {}", "C:/logs"), "Logs: C:/logs");

        set_language(Lang::PtBr);
        assert_eq!(tr("Settings"), "Configurações");
        assert_eq!(tr("Scrollback lines"), "Núm. de Linhas de Rolagem");
        assert_eq!(
            tr("A string nobody has translated"),
            "A string nobody has translated"
        );
        assert_eq!(tr1("Saved in {}", "x.toml"), "Salvo em x.toml");
        // Left as it was found, so the tests that follow are not at the mercy
        // of the order they run in.
        set_language(Lang::En);
    }
}
