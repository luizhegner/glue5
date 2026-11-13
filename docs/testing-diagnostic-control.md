# Diagnóstico por IA: roteiro de validação

Alvo: Galaxy S21+ SM-G996B / Exynos 2100 / Mali-G78. A identificação S22 da
ROM não muda o aparelho alvo. Não executar builds ou testes Rust no Inspiron.

Este roteiro não é um pedido para gerar outra build. Primeiro conferir o CI
completo da revisão candidata. A aprovação de testes não comprova o aparelho.

## 1. Preparação sem conceder controle

- Abrir Settings > Diagnostics > Diagnóstico por IA. A opção deve começar
  desligada após encerrar o processo e reabrir o app.
- Tocar no texto da linha ou no interruptor: ambos devem alternar uma única vez.
- Ativar suporte a mods/conexão, se indicado. Se a sessão começou sem mods,
  reiniciar o jogo com essa opção habilitada; não reinstalar nem limpar cache.
- O app deve distinguir serviço iniciando, rede local desativada, ausência de
  pareamento e necessidade de voltar ao jogo. Pareado não significa conectado.
- Escolher Codex (TOML) ou Genérico (JSON). Criar e copiar deve ser uma ação
  explícita, não um efeito de abrir ou renderizar a tela.

## 2. Conectar o agente de confiança

No Codex, a configuração MCP usa `config.toml`; servidores HTTP aceitam
`http_headers`. O app desktop também oferece Settings > MCP servers para
gerenciar servidores. Conferir o procedimento na
[documentação oficial](https://learn.chatgpt.com/docs/extend/mcp?surface=cli).

- Usar a configuração copiada no arquivo **privado** do cliente. Não colar
  tokens no chat, em logs, no GitHub ou no projeto. Preservar as outras seções.
- Se já existe `mcp_servers.glue5`, atualizar essa seção em vez de duplicá-la.
  Recarregar/reiniciar a conexão conforme o cliente. Copiar pelo celular não
  configura automaticamente o computador nem esta conversa.
- Usar a rede privada de confiança. Não abrir porta pública do Studio.
- Conferir descoberta de `diagnostic_control` e `logs`. No formato Codex, o
  cliente só habilita essas duas ferramentas e mantém prompts de aprovação.
- O token ainda tem privilégios Studio: a lista do cliente não é um sandbox.
  Revogar o cliente em Mods > Studio > Paired quando não for mais necessário.
- Não instalar uma IA ou adicionar chaves de serviços externos ao GLUE5.

## 3. Solicitação e recusa primeiro

- Salvar o progresso. Não existe sandbox de saves ou bloqueio de autosave.
- Voltar ao jogo, sem o menu nativo aberto. O agente solicita controle pela
  ferramenta `diagnostic_control`, operação `request`.
- Deve aparecer o nome do cliente e a autorização local. Recusar primeiro:
  não deve aparecer aviso de posse nem ser aceito comando remoto.
- Solicitar novamente e deixar expirar; confirmar depois não deve ativar
  uma solicitação vencida. O código bruto de pareamento não basta para controlar.

## 4. Sessão autorizada e parada

- Autorizar uma solicitação nova. Conferir aviso exato “A IA está controlando
  o jogo”, Parar IA acessível e radar original no topo esquerdo sem alteração.
- Testar inicialmente **sem acelerar, atirar ou mover o personagem**. Manter
  o lease com heartbeat. Ações têm no máximo 1 segundo; lease expira em 5 segundos.
- Toque, teclado, mouse, controle e sensores locais não devem mover o jogador.
  Navegação do Android e emergências não devem ser bloqueadas.
- Parar IA, Voltar/Esc e Back+Start no mesmo controle devem encerrar a sessão.
  A combinação física deve funcionar mesmo com outros comandos segurados.
- Testar perda de heartbeat, segundo plano e revogação do cliente separadamente.
  Nenhum acelerador, mira, botão ou vibração pode continuar preso.
- O app tenta pausar quando possível, mas devolve os controles sem aguardar
  o jogo aceitar a pausa. Não deve fechar um menu de pausa que já estava aberto.
- Reconectar não pode retomar a sessão antiga. Reativação exige novo pedido e
  nova autorização. Desabilitar a opção local cancela o controle.

## 5. Limites desta entrega

Não há observação de imagem, vídeo em tempo real ou reprodução autônoma de bugs.
O agente só recebe os resultados das ferramentas disponíveis. `logs` lê logs
do Studio/mods; não é uma captura geral do Android. Shizuku tem autorização e
estado próprios e não deve iniciar por ativar diagnóstico de IA.

Registrar resultado por item sem credenciais ou conteúdo de saves. Não marcar
“validado no aparelho” só porque a tela aparece ou a compilação passou.
