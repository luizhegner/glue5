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

### Navegação do repositório por agentes de código
- O usuário esclareceu que a ideia de remover ou reduzir alvos iOS/macOS/Windows/Linux é para facilitar a busca e a análise do repositório por LLMs, não para mudar o produto suportado. A alternativa discutida foi melhorar o mapa de arquitetura/instruções do repositório e indicar código legado, preservando o servidor Windows e evitando apagar código compartilhado sem necessidade. Ainda sem decisão de remover plataformas.

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
