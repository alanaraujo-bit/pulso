# Pulso

Painel de bateria para periféricos sem fio no Windows 11. Mostra nível, carregamento, tempo ligado,
tempo desde a última carga e estimativa de autonomia de cada dispositivo, num painel de vidro
(acrylic do próprio Windows) que abre ao lado do relógio ou fica flutuando na área de trabalho.

- **Bandeja**: um indicador com a porcentagem de cada dispositivo ao lado do relógio
  (verde carregando, vermelho em 15% ou menos, apagado em repouso). Clique abre o painel.
- **Flyout**: abre ancorado no ícone clicado, fecha com Esc ou ao clicar fora.
- **Painel flutuante**: arrastável, posição lembrada, opcionalmente sempre visível.
- **Alertas**: notificação em 15% e 5% e quando a carga completa (uma vez por ciclo).
- **Leve**: em repouso o app é só o processo Rust (~3 MB de RAM, ~0,1% de CPU). O WebView do painel
  é criado ao abrir (~300 ms a frio, ~30 ms a quente) e destruído após 3 minutos fechado.

## Dispositivos suportados

| Protocolo | Transporte | Testado com |
|---|---|---|
| Logitech HID++ 2.0 (features 0x1004 / 0x1000 / 0x1001) | Receptores LIGHTSPEED, Unifying, Bolt e cabo | PRO 2 LIGHTSPEED (receptor C543) |
| Logitech Centurion (bridge 0x0003, bateria 0x0104) | Dongle de headset, usage page 0xFFA0 | PRO X 2 LIGHTSPEED (0AF7) |
| CompX 2.4G (report 0x13, sessão + push) | Dongle 3554:FA09 | AULA F75 (código de modelo 0x00CD) |

Os três protocolos são somente leitura: o Pulso só consulta bateria e nome, nunca grava configuração
no dispositivo. Funciona junto com o G HUB aberto (usa um software ID HID++ próprio).

## Como as estimativas funcionam

`src/estimate.rs` guarda o histórico de cada dispositivo em `%APPDATA%\Pulso\history.json`.

- O consumo é medido contra **tempo ativo** (segundos em que o dispositivo respondeu), não contra o
  relógio. Um headset desligado à noite não "parece" gastar menos.
- A estimativa mistura a autonomia oficial do modelo (tabela `prior`) com a taxa observada nas
  últimas 24 h ativas da descarga atual, ponderada por quanto já foi medido. Dispositivo novo mostra
  a especificação; com o uso, converge para o seu padrão real (passe o mouse sobre "Restante" para ver
  quanto da estimativa já vem da medição).
- Tempo até carga completa usa a taxa medida na carga atual, com a desaceleração típica de Li-ion
  acima de 85%.
- "Ligado há" só aparece quando o Pulso viu o dispositivo ligar; antes disso o painel mostra
  "Ativo há" (desde que o Pulso o encontrou).

## Estrutura

```
src/hidpp.rs      HID++ 2.0 (Logitech mouse/teclado), incl. notificação 0x41 de conexão
src/centurion.rs  Centurion (headsets Logitech recentes)
src/compx.rs      dongles CompX 2.4G (AULA e afins)
src/monitor.rs    thread que segura os handles HID, faz polling lento e escuta eventos
src/estimate.rs   histórico, estimativas e alertas (com testes)
src/sys.rs        Win32: glifo da bandeja (GDI), tema, autostart, promoção do ícone, cantos DWM
src/main.rs       app Tauri: bandeja, janela, comandos
ui/               painel (HTML/CSS/JS puro, sem framework)
dev/              preview do painel com dados falsos (node dev/serve.mjs)
```

## Desenvolvimento

Requisitos: Rust (MSVC), WebView2 (já vem no Windows 11), Node só para empacotar.

```powershell
cargo test --lib                      # testes de protocolo e estimativa
cargo run --bin probe                 # lista e lê todos os dispositivos (diagnóstico)
cargo run                             # app em modo debug (imprime as leituras no console)
npx @tauri-apps/cli@2 build           # release + instalador NSIS em target/release/bundle/nsis
```

Abrir o `pulso.exe` com ele já rodando abre o painel da instância existente.

## Créditos de protocolo

Layouts de protocolo aprendidos da documentação pública de
[HeadsetControl](https://github.com/Sapd/HeadsetControl) (Centurion) e
[vigil-watt](https://github.com/evren-oct/vigil-watt) (CompX/AULA). Nenhum código copiado.
