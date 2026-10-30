# GLUE 5 Android Debug (Windows)

Protótipo de uma interface Windows para coletar logs do GLUE 5 por ADB sobre Wi-Fi e montar um ZIP para anexar num relato de bug.

## Requisitos

- Windows 10/11 com Windows PowerShell 5.1 e WinForms (já incluídos no Windows).
- `adb.exe` do Android SDK Platform Tools. O app tenta achá-lo no `PATH` e no SDK padrão; também permite selecioná-lo.
- No Android, ativar **Opções do desenvolvedor → Depuração sem fio**. O computador e o aparelho devem estar na mesma rede.

## Uso

1. Execute `Glue5-Debug.cmd`.
2. No Android, abra a tela **Depuração sem fio → Parear dispositivo com código**. Informe no app o IP, a porta de pareamento e o código temporário.
3. Use a porta indicada em **Endereço IP e porta** para conectar. Atualize e selecione o aparelho na lista.
4. Clique **Iniciar captura**. O app abre o GLUE 5 se necessário e acompanha o processo Android `local.glue5`.
5. No telefone, tente iniciar o jogo e reproduza o erro.
6. Volte ao Windows e clique **Parar e montar ZIP**. O pacote fica em `Documentos\GLUE5-Debug`.
7. Revise o ZIP e envie-o junto do relato. Não precisa de screenshot.

## O que entra no pacote

- Logcat em tempo real filtrado pelo PID do processo `local.glue5`.
- Buffer Android de crash filtrado pelo mesmo PID, quando disponível.
- Versão do app, versão/API Android e ABI principal.
- Erros do próprio ADB durante a captura.
- `game.log` somente se a caixa opcional estiver marcada **e** o Android permitir acesso ao caminho externo conhecido.

A ferramenta não limpa o logcat, não executa `bugreport`, não inclui o serial do aparelho, IP, porta ou código de pareamento e não coleta logs de outros apps de propósito. Logs do GLUE 5 ainda podem conter caminhos ou outros valores técnicos: revise o ZIP antes de compartilhar. O código temporário de pareamento é usado apenas no comando ADB e apagado do campo após sucesso.

Este protótipo é um script PowerShell local, não um executável assinado. O pareamento depende da tela e das permissões do Android; alguns dispositivos podem exigir reconexão depois de reiniciar a depuração sem fio.
