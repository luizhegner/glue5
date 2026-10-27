# Diário do projeto (temporário)

Registro de requisitos, observações e decisões do GLUE 5. **Não incluir dados pessoais, identificadores ou informações de conta.** Este arquivo é temporário e deve ser removido ao final do trabalho, quando solicitado.

## 2026-10-27 — Diagnóstico Android e regressões

### Observações reportadas
- Ao tentar iniciar o jogo, o APK apresenta `wasm trap: unreachable instruction executed`.
- A tela de controles não mostra as novidades esperadas.
- Em Settings → Game, desapareceram as opções de importar save e copiar o relatório de diagnóstico.
- Não há uma opção acessível no app para obter `game.log`; a falha impede depurar pelo próprio GLUE 5.
- O APK em questão foi compilado pelo usuário; não presumir que veio do workflow do repositório.

### Requisitos e sugestões
- Desenvolver uma ferramenta para Windows que use ADB por Wi-Fi para depurar especificamente `local.glue5` e gere um arquivo de diagnóstico fácil de revisar e enviar.
- Capturar logs do processo GLUE 5 durante uma reprodução iniciada pelo usuário; não coletar o logcat inteiro do dispositivo por padrão.
- Evitar exportar serial do aparelho, IP, código de pareamento, contas ou outros identificadores. Informar que logs podem conter caminhos e permitir revisão antes do envio.
- Preservar as opções já existentes. HUD, touchscreen e importação de save/jogo só contam como concluídos se as funções anteriores continuarem operando.
- Manter um diário das ordens, sugestões, observações e decisões relevantes ao projeto, sem dados pessoais; apagar este diário no encerramento do projeto, mediante solicitação.

### Acompanhamento técnico
- No código, o layout touchscreen `Modern` passou a ser o padrão em dispositivos móveis; ações secundárias ficam no botão `More`. `Classic` mantém as ações mais visíveis. Tornar essa diferença clara e verificar se ela explica controles percebidos como ausentes.
- O botão de diagnóstico Android estava condicionado a `started.is_none()` e foi movido para continuar visível depois que o jogo inicia. Para cobrir o encerramento por trap fatal, o código atual também monta o relatório em `stopped()` e solicita sua gravação/cópia antes de sair; isso ainda precisa de build e teste em aparelho.
- A integração GitHub não permitiu disparar o workflow Android (`403 Resource not accessible by integration`). O usuário vai iniciar o workflow manualmente; não afirmar que um APK foi gerado nesta sessão.
- O trap ainda não foi reproduzido nem explicado. O relatório interno agora conserva o detalhe de trap disponível no runtime, mas não substitui o logcat completo; avaliar a captura externa se faltar contexto.
- Criado um protótipo Windows em `tools/glue5-debug/`: pareamento/conexão ADB por Wi-Fi, captura do processo `local.glue5` e exportação de ZIP. A ausência do PID foi tratada sem falhar a captura, mas o protótipo não foi executado em Windows nem validado contra um aparelho; `game.log` é opcional porque ADB pode não ter acesso.

## 2026-10-27 — Captura automática e seletor de jogo

- O usuário propôs Shizuku/ADB Wi-Fi no próprio aparelho. Primeiro foi escolhida uma captura interna do trap; depois o usuário esclareceu que isso não basta e pediu explicitamente o logcat completo via Shizuku. A captura interna continua como relatório curto, mas o mecanismo Shizuku agora é requisito e está em implementação local.
- Após reinstalar e limpar os dados, `Find files` não abriu/selecionou um jogo. Corrigir erros silenciosos, não cair no seletor desktop no Android, limpar resultados antigos e deixar explícito por que a seleção falhou.
- O usuário vai executar **Build → Run workflow → Android** manualmente após as mudanças; não alterar pipeline/assinatura e não alegar que o build em aparelho passou até receber o resultado.
- Implementação local em andamento: o hook de trap conserva código/linha/frames nativos; `stopped()` gera o relatório automaticamente; Android salva `glue5-support-report.txt` e tenta copiar para clipboard antes de sair. `Find files` agora deve retornar sem fallback desktop no Android e registra resultados/cancelamentos. Ainda sem build de verificação.
- Manter como prioridade recuperar inicialização do jogo e opções existentes antes de considerar HUD/touch/importações concluídos.
- O usuário corrigiu o requisito de diagnóstico: precisa do **logcat completo do aparelho**, não apenas do relatório interno. Solicitou que, ao detectar falha na sessão anterior, o GLUE 5 pergunte se deve capturar/copy, com integração Shizuku já instalado e iniciado por Depuração sem fio.
- Implementação local: a API oficial Shizuku/UserService, pedir autorização explícita para o GLUE 5, executar `logcat -d -b all -v threadtime`, salvar a captura completa, copiar para clipboard quando couber e permitir compartilhar o arquivo quando exceder o limite seguro do clipboard. Falha/encerramento inesperado abre uma confirmação na sessão seguinte, com ação disponível na ficha LAST SESSION. O GLUE 5 não cria nem lê códigos de pareamento; o usuário inicia/parea o serviço no próprio Shizuku. Ainda sem build/teste.

### Retrospectiva — regressão de Find files
- Sim: alterei um fluxo Android que já existia. Em `e6c5c3c`, `Action::FindGame` deixou de chamar diretamente `find_game(None)` (revarredura dos caminhos conhecidos) e passou a abrir um picker de pasta; para pastas fora do diretório do app, esse caminho exige o acesso especial `MANAGE_EXTERNAL_STORAGE`. Uma reinstalação limpa revoga essa autorização e a aplicação precisa guiá-la de novo.
- Em `b852d82`, também removi o fallback para a revarredura antiga quando a chamada/picker Android falha. Isso deixou o novo caminho menos tolerante e pode explicar o botão não encontrar o jogo quando a permissão não foi concedida ou a Activity/picker não abriu.
- Não posso afirmar qual desses casos ocorreu no aparelho sem o logcat: o root cause em runtime continua desconhecido. O erro foi não preservar e testar o comportamento que já funcionava antes de substituir o fluxo.
- Reintroduzi no working tree a revarredura dos caminhos conhecidos quando o picker falha ou retorna recusa; o picker segue como caminho principal. Ainda não compilado/testado em dispositivo.
- A Build run 40452436632 falhou em `:compileReleaseAidl`: `ILogcatCaptureService.aidl` atribuía transação só a `destroy()` e não a `captureLogcat()`. Corrigi o contrato atribuindo `= 1` a `captureLogcat()` (commit `4b12ae0`); falta rerodar o Build para confirmar a compilação.

### Navegação do repositório por agentes de código
- O usuário esclareceu que a ideia de remover ou reduzir alvos iOS/macOS/Windows/Linux é para facilitar a busca e a análise do repositório por LLMs, não para mudar o produto suportado. A alternativa discutida foi melhorar o mapa de arquitetura/instruções do repositório e indicar código legado, preservando o servidor Windows e evitando apagar código compartilhado sem necessidade. Ainda sem decisão de remover plataformas.

## 2026-10-27 — Material 3 Expressive e captura manual de logcat

### Escopo autorizado
- Antes da futura reformulação visual inspirada em GTA V, melhorar os menus existentes com hierarquia, agrupamento, cor/forma tonal e ícones em linguagem Material You 3 Expressive. Esta etapa é transitória e não redesenha o produto como GTA V.
- Adicionar captura manual contínua do logcat completo do aparelho via Shizuku, com ações distintas para iniciar e finalizar; conservar a captura do log após falha de sessão.
- Declarar explicitamente que o logcat cobre o dispositivo inteiro e pode conter dados de outros apps; exigir confirmação do usuário e autorização do Shizuku antes de iniciar. Guardar o arquivo final e oferecer compartilhamento/cópia segura.

### Plano e critérios de validação
1. Nas páginas Settings atuais, acrescentar cabeçalho/tabs expressivos, ícones vetoriais sem dependência de fontes específicas e grupos sem alterar o significado dos controles; garantir layout em telefone, tablet, desktop e safe area.
2. Acrescentar uma seção Android de diagnóstico com aviso de privacidade e botões Iniciar/Finalizar. No serviço Shizuku, manter `logcat` contínuo por um pipe até stop, gravar incrementalmente no armazenamento privado do app e só então apresentar compartilhar/copy-tail. Manter separado o snapshot `-d -b all` usado para a sessão anterior.
3. Validar `cargo fmt --check` e testes da UI/crates relevantes; compilar o AIDL/Java/Android. No aparelho, confirmar fluxo sem Shizuku, autorização negada/concedida, start→stop com arquivo compartilhável e a captura automática após falha. Não marcar teste em aparelho como concluído sem resultado do usuário.
4. Não mexer no pipeline/assinatura. O usuário executará o workflow Android manualmente depois que o código estiver pronto.

### Riscos a observar
- Logcat completo contém potencialmente dados sensíveis e cresce enquanto a captura está ativa; não compartilhar automaticamente e deixar claro quando a captura terminou.
- A implementação deve fechar o serviço/pipe mesmo em cancelamento, falha de binder ou saída inesperada da Activity, sem interromper a captura por falha de sessão.

### Implementação local desta etapa
- Settings agora recebe tokens de superfície/acento Material 3 Expressive, tabs segmentadas, títulos de página, cabeçalhos de grupo e ícones vetoriais próprios (sem fonte de emoji adicional). Ações recebem tratamento tonal distinto dos controles de ajuste; as funções dos controles existentes não foram substituídas.
- No Android, foi adicionada a aba Logs, disclosure de privacidade, confirmação antes da captura e botões Start capture / Finish and save. O stop solicita ao UserService que encerre `logcat`; o GLUE 5 grava incrementalmente em arquivo privado e, ao terminar, oferece compartilhar ou copiar de forma explícita. Snapshot `-d -b all` continua atendendo LAST SESSION/recuperação de falha.
- AIDL foi expandida com `startLogcat` e `stopLogcat`; serviço versão 2 mantém processo contínuo separado do snapshot. A autorização e o pareamento continuam sendo iniciados pelo usuário no Shizuku; o GLUE 5 não manipula códigos.
- **Validação pendente:** este sandbox não tem `cargo`, `rustc`, `rustfmt`, `java`, `javac`, Gradle nem wrapper Gradle; portanto não rodei formato, testes Rust, compilação AIDL/Android ou aparelho. `git diff --check` passou sem avisos. Não declarar compilação ou teste Shizuku concluídos; após revisar o patch, o usuário executará o workflow e o teste em dispositivo.

## 2026-10-27 — Próxima etapa: controles, HUD e UI

### Novos requisitos do usuário
- Editor de controles de toque na tela, com suporte a carregar presets XML; por enquanto a prioridade é permitir que o próprio usuário edite os controles.
- A interação com o celular do GTA deve ser por toques diretos: tocar em cada app, iniciar chamada e encerrá-la; não depender de gestos. Remover os direcionais.
- Os controles atuais cobrem o minimapa/HUD sob o polegar esquerdo ao usar o joystick. Avaliar mover o minimapa para um canto superior ou permitir posicionamento/configuração.
- Separar o HUD do GTA da renderização 3D para que escala/responsividade do HUD possa ser tratada independentemente. O HUD atual está pequeno e pouco legível apesar do jogo usar a resolução total do aparelho.
- Reformular a UI do GLUE 5 com linguagem visual coerente com GTA V, reduzindo a ruptura visual na transição; a separação do HUD deve ser considerada parte da solução, não uma mudança isolada.
- Há um piscar azul de milissegundos perto do minimapa em alguns momentos. A relação com mudanças futuras do HUD ou dos controles é apenas hipótese; não atribuir causa antes de isolar.
- Não fazer alterações sem rumo: inspecionar o caminho atual, elaborar plano, registrar riscos, estabelecer teste e anotar progressos/regressões.
- Este pedido autoriza explicitamente voltar a trabalhar em controles touchscreen e na aparência/background/UI, escopos que anteriormente estavam pausados.

### Plano de investigação, sem implementação ainda
1. Build Android run 40452436632, SHA `7be19d1`, terminou em falha no step `Build`; setup, dependências e assinatura passaram. A annotation só informa exit code 1, e a coleta dos logs via API falhou com EOF; a causa permanece desconhecida. O aviso de Node.js 20 é não fatal. Obter as linhas do erro antes de corrigir; esta build não inclui as mudanças desta próxima etapa.
2. Registrar baseline de controles e renderização no aparelho. Separar os bugs de toque, escala do HUD e piscar azul; variar um subsistema por vez.
3. Mapear o modelo de controles existente e definir XML de preset com validação, versão, limites de posição/tamanho e fallback seguro; só então desenhar o editor mínimo.
4. Usar o censo GPU e as medições existentes para decidir a separação do HUD. Gate: comprovar que alvos, leituras do back buffer, `SV_Position`, profundidade, clears e limites de pass são seguros; sem redirect especulativo nem descarte previsto.
5. Fazer protótipo isolado de HUD responsivo e layout sem oclusão do polegar; comparar capturas/métricas antes e depois e testar a cintilação azul com toggles A/B.
6. Reformular a UI do GLUE 5 depois de estabilizar a base técnica, com estilo próprio inspirado na hierarquia visual do GTA V e transições consistentes; validar em tela de celular e sem jogo instalado.
7. Para cada etapa, registrar commit, build, aparelho/ação de teste, resultado e qualquer regressão antes de avançar.

## Requisitos técnicos registrados anteriormente

- Estender E0 e o censo GPU existente; não reconstruir a infraestrutura já compilada no CI.
- `pass swaps` mede trocas de conjunto de render targets; passes que escrevem no back buffer precisam de uma métrica separada.
- `reads SV_Pos: 3` conta programas/labels distintos no censo, não leituras por frame; o gate associado ainda não passou.
- A fórmula `10,4 MB × (1 + 2·(P−1))` é apenas estimativa teórica; para `P=4`, cerca de `73 MB`, não uma medição.
- Nunca ignorar `CLEAR_RT` do back buffer na fase HUD; se aparecer, cancelar o redirecionamento naquele frame.
- Não usar `StoreOp::Discard` baseado em previsão.
- Validar a forma WGSL via IR do naga, não por busca textual. O seed contém descritores, não milhares de fontes WGSL.
- Não presumir que o pass de HUD termina em `SCENE_DONE`; medir/comprovar antes de redirecionar.
- Em relatório de bugs, preferir dados copiáveis a screenshots. Não incluir conteúdo de saves nem coordenadas cruas de touch.
- Preservar alterações locais preexistentes e não alterar pipeline ou assinatura Android sem pedido explícito.
