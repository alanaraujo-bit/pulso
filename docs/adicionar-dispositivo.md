# Adicionando um dispositivo

Este guia mostra como ensinar o Pulso a ler um dispositivo novo, de "não aparece" até o PR
aceito. Não precisa fazer tudo: um pedido de dispositivo com uma boa saída do probe (passos 1 e 2)
já é uma contribuição de verdade. ([English version](adding-a-device.md))

## 1. Veja o que o Windows enxerga

Ligue o dispositivo, acorde ele (mexa, aperte uma tecla) e rode:

```powershell
cargo run --bin probe -- --all
```

O primeiro bloco lista todas as coleções HID da máquina:

```
046d:c543  if=2   ff00:0002  USB Receiver
3554:fa09  if=1   ff02:0002  2.4G Wireless Receiver
```

Desconecte o receptor, rode de novo e compare: as linhas que sumiram são do seu dispositivo.
A bateria quase sempre fica numa **usage page do fabricante** (`ff00` a `ffff`).

O probe nunca escreve em dispositivos que ele ainda não suporta, então é seguro rodar.

## 2. Confirme que a bateria pode ser lida

O Pulso só lê o que o dispositivo informa. Confira se o software oficial (ou o driver web do
fabricante) mostra a bateria. Se nada oficial consegue ver, não há o que ler.

Nesse ponto, abra um [pedido de dispositivo](https://github.com/alanaraujo-bit/pulso/issues/new?template=device-support.yml)
com a saída do probe, o modelo exato, o tipo de conexão e o que o app oficial mostra.

## 3. Descubra o protocolo

Do mais fácil ao mais trabalhoso:

1. **Protocolo conhecido, produto novo.** Muitas marcas usam o mesmo chip. A Logitech usa HID++ ou
   Centurion, e muitos teclados 2.4G chineses usam dongles CompX (`3554:*`). Teste o módulo que já
   existe: muitas vezes basta adicionar um product id ou um código de modelo.
2. **Projetos open source.** Procure no GitHub pelo VID:PID (`"3554" "fa09"`, `"0x0af7"`).
   Projetos como HeadsetControl, Solaar, OpenRGB e vigil-watt documentam muitos protocolos.
   Aprenda o formato dos frames pela documentação e pelos testes deles; não copie código GPL para
   este projeto MIT.
3. **Drivers web.** Muitos fabricantes têm um site com driver WebHID. O JavaScript dele mostra a
   consulta de bateria e o formato da resposta.
4. **Captura USB.** Instale o [USBPcap](https://desowin.org/usbpcap/) e o Wireshark, abra o app
   oficial e procure a requisição que vem antes de uma resposta contendo a porcentagem (procure o
   valor atual da bateria como byte, por exemplo `0x56` para 86%). Mude o estado de carga e capture
   de novo para achar o bit de carregando.

Anote: report id, tamanho do report, os bytes da requisição, os bytes da resposta, qual byte é a
porcentagem e qual bit indica carregando. Capture frames reais em vários níveis e carregando.

## 4. Implemente

Crie `src/<protocolo>.rs` seguindo os módulos existentes (o `compx.rs` é o menor):

- Um predicado `is_<coisa>(d: &hidapi::DeviceInfo) -> bool` que confere VID, PID, usage page e usage.
- Um `parse(...)` puro que transforma os bytes da resposta em
  `BatterySample { percent, charge, voltage_mv }`. Rejeite porcentagem acima de 100. Dispositivos
  dormindo costumam responder `0` ou `1`: trate isso como repouso.
- Uma struct pequena com o `HidDevice`, com `read_battery()` e, se o dispositivo manda atualizações
  sozinho, `pump(wait)` para recolher.
- Testes com os **frames reais** capturados, em hex.

Depois conecte:

- `src/lib.rs`: `pub mod <protocolo>;`
- `src/monitor.rs`: abra as coleções no bloco de enumeração e adicione uma função
  `poll_<coisa>` que emite `Reading`s (veja `poll_dongle`). Use um `id` estável: ele é a chave do
  histórico de bateria salvo em disco.
- `src/estimate.rs`: adicione o modelo em `prior()` com a autonomia oficial (horas) e a velocidade
  de carga (% por hora) da ficha técnica.
- `src/bin/probe.rs`: imprima o dispositivo novo para outras pessoas testarem.

## 5. Teste no hardware e abra o PR

- `cargo run --bin probe` lê o dispositivo.
- `cargo run`: o dispositivo aparece no painel e na bandeja, dorme e acorda direito, e o estado
  muda para carregando quando você liga o cabo.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib`.

No PR, liste o hardware testado, os frames usados e o que não deu para verificar
(por exemplo "ainda não vi o estado de carga completa").
