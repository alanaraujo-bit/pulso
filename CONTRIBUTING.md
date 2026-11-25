# Contribuindo com o Pulso

Valeu por ajudar! Issues e pull requests são bem-vindos em **português ou inglês**.

## Formas de ajudar

| Você tem | Faça isso |
|---|---|
| Um dispositivo que o Pulso não lê | Abra um [pedido de dispositivo](https://github.com/alanaraujo-bit/pulso/issues/new?template=device-support.yml) com a saída do probe. Não precisa programar. |
| Um dispositivo com leitura errada (porcentagem, carregando, repouso) | Abra um [relato de bug](https://github.com/alanaraujo-bit/pulso/issues/new?template=bug.yml) comparando com o app oficial. |
| Tempo para programar | Pegue uma issue com [`good first issue`](https://github.com/alanaraujo-bit/pulso/labels/good%20first%20issue) ou [`help wanted`](https://github.com/alanaraujo-bit/pulso/labels/help%20wanted), ou adicione um protocolo seguindo [o guia](docs/adicionar-dispositivo.md). |
| Talento para design ou idiomas | Traduções, acabamento do painel, ícones. Comece uma conversa nas Discussions. |

## Ambiente

- Windows 11 e [Rust](https://rustup.rs) com a toolchain MSVC.
- Node.js só se quiser gerar o instalador.

```powershell
git clone https://github.com/alanaraujo-bit/pulso
cd pulso
cargo test --lib
cargo run              # build de debug, imprime cada leitura no console
```

Rode um Pulso por vez: abrir de novo só mostra o painel da instância que já está rodando.

## Estrutura do projeto

```
src/
  hidpp.rs       Logitech HID++ 2.0 (receptores e dispositivos com cabo)
  centurion.rs   Logitech Centurion (headsets LIGHTSPEED recentes)
  compx.rs       dongles 2.4 GHz CompX (AULA e outros)
  monitor.rs     thread em segundo plano: handles HID, polling lento, escuta de eventos
  estimate.rs    histórico por dispositivo, modelo de autonomia e carga, alertas
  model.rs       tipos compartilhados (Reading, BatterySample, Kind, Charge)
  sys.rs         Win32: desenho do ícone da bandeja, tema, autostart, promoção do ícone, cantos DWM
  main.rs        app Tauri: ícones da bandeja, ciclo de vida do painel, comandos
  bin/probe.rs   diagnóstico de hardware
ui/              painel (HTML, CSS e JS puros)
dev/             preview do painel com dados falsos, para mexer na interface sem hardware
docs/            guias e imagens do README
```

Fluxo dos dados: os módulos de protocolo transformam reports HID em `Reading`s, o `monitor.rs`
os emite, o `estimate.rs` os transforma em `DeviceView`s e o `main.rs` entrega isso para a
bandeja e para o painel.

## Diretrizes

- **Mudanças pequenas e completas.** Resolva o problema inteiro com o mínimo de código novo.
  Reaproveite o que já existe no projeto antes de adicionar uma dependência.
- **Código de protocolo é somente leitura.** Nunca envie comandos que mudem configurações do
  dispositivo. Se a consulta de bateria exigir uma escrita (como abrir uma sessão), explique o
  porquê no cabeçalho do módulo.
- **Todo parser tem teste com frames reais** capturados do hardware. Coloque o hex no teste.
- **Nunca chame API de janela segurando o mutex de estado** no `main.rs`: mover ou redimensionar a
  janela dispara eventos síncronos cujos handlers pegam o mesmo lock.
- **Comentários explicam o porquê**, não o quê.
- Mantenha leve: o app em repouso precisa continuar sendo um processo pequeno. Meça RAM e CPU se
  a mudança mexer no loop do monitor ou no ciclo de vida do painel.

Antes de abrir o PR:

```powershell
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --lib
```

O CI roda os mesmos três no Windows.

## Commits e pull requests

- [Conventional Commits](https://www.conventionalcommits.org/pt-br/): `feat:`, `fix:`, `docs:`,
  `refactor:`, `chore:`. Exemplo: `feat(compx): ler bateria do MCHOSE G75`.
- Um assunto por PR. Descreva o que testou e em qual hardware.
- Mudança visual: inclua print de antes e depois.

## Versões

Os mantenedores criam a tag `vX.Y.Z`; o workflow de release gera o instalador e publica no
GitHub Releases.

## Código de conduta

Seja gentil e assuma boa-fé. Veja o [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
