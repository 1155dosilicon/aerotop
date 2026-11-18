#!/bin/sh
# Instala o AeroTop para o usuário atual (em ~/.local), sem precisar de root.
#
#   ./install.sh              instala (compila se tiver Rust, senão baixa o programa pronto)
#   ./install.sh --uninstall  remove tudo que foi instalado
#
# Também funciona direto da internet:
#   curl -fsSL https://raw.githubusercontent.com/1155dosilicon/aerotop/main/install.sh | sh
set -eu

REPO="1155dosilicon/aerotop"
RAW="https://raw.githubusercontent.com/$REPO/main"
BIN_URL="https://github.com/$REPO/releases/latest/download/aerotop"

DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$DATA/applications"
ICON_DIR="$DATA/icons/hicolor"
SIZES="16 32 48 64 128 256 512"

say() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merro:\033[0m %s\n' "$*" >&2; exit 1; }

baixar() { # baixar URL DESTINO
    if command -v curl >/dev/null 2>&1; then curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then wget -q "$1" -O "$2"
    else die "preciso do curl ou do wget para baixar $1"; fi
}

atualizar_menu() {
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$APP_DIR" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$ICON_DIR/index.theme" ] \
        && gtk-update-icon-cache -q -t "$ICON_DIR" 2>/dev/null || true
    touch "$ICON_DIR" 2>/dev/null || true
}

desinstalar() {
    say "Removendo o AeroTop"
    rm -f "$BIN_DIR/aerotop" "$APP_DIR/aerotop.desktop" "$ICON_DIR/scalable/apps/aerotop.svg"
    for n in $SIZES; do rm -f "$ICON_DIR/${n}x${n}/apps/aerotop.png"; done
    atualizar_menu
    say "Pronto. O cache em ~/.cache/aerotop (dados da RAM) foi mantido; apague à mão se quiser."
    exit 0
}

[ "${1:-}" = "--uninstall" ] && desinstalar
[ "$(uname -s)" = "Linux" ] || die "o AeroTop só funciona no Linux"

# Rodando de dentro do repositório ou pela internet (curl | sh)?
SRC=""
case "$0" in
    */install.sh|install.sh) d=$(cd "$(dirname "$0")" && pwd)
        [ -f "$d/Cargo.toml" ] && [ -f "$d/assets/aerotop.svg" ] && SRC="$d" ;;
esac
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR/scalable/apps"

# 1. Programa
if [ -n "$SRC" ] && { command -v cargo >/dev/null 2>&1 || [ -x "$HOME/.cargo/bin/cargo" ]; }; then
    CARGO=$(command -v cargo || echo "$HOME/.cargo/bin/cargo")
    say "Compilando com o Rust (pode levar um minuto)"
    (cd "$SRC" && "$CARGO" build --release)
    install -m 755 "$SRC/target/release/aerotop-rs" "$BIN_DIR/aerotop"
else
    [ "$(uname -m)" = "x86_64" ] || die "o programa pronto é só para x86_64; instale o Rust (https://rustup.rs) e rode de novo"
    G=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
    if [ -n "$G" ] && [ "$(printf '%s\n2.34\n' "$G" | sort -V | head -1)" != "2.34" ]; then
        die "o programa pronto precisa da glibc 2.34+ (você tem $G); instale o Rust (https://rustup.rs) e rode de novo para compilar"
    fi
    say "Baixando o programa pronto"
    baixar "$BIN_URL" "$TMP/aerotop"
    install -m 755 "$TMP/aerotop" "$BIN_DIR/aerotop"
fi

# 2. Ícone e atalho do menu
if [ -n "$SRC" ]; then
    cp "$SRC/assets/aerotop.svg" "$TMP/aerotop.svg"
    cp "$SRC/assets/aerotop.desktop" "$TMP/aerotop.desktop"
    cp -r "$SRC/assets/png" "$TMP/png"
else
    say "Baixando ícone e atalho"
    baixar "$RAW/assets/aerotop.svg" "$TMP/aerotop.svg"
    baixar "$RAW/assets/aerotop.desktop" "$TMP/aerotop.desktop"
    mkdir -p "$TMP/png"
    for n in $SIZES; do baixar "$RAW/assets/png/aerotop-$n.png" "$TMP/png/aerotop-$n.png"; done
fi

install -m 644 "$TMP/aerotop.svg" "$ICON_DIR/scalable/apps/aerotop.svg"
for n in $SIZES; do
    mkdir -p "$ICON_DIR/${n}x${n}/apps"
    install -m 644 "$TMP/png/aerotop-$n.png" "$ICON_DIR/${n}x${n}/apps/aerotop.png"
done

# Abre numa janela de 120x40 (o tamanho ideal) quando reconhece o terminal da área de trabalho.
EXEC="$BIN_DIR/aerotop"
case "${XDG_CURRENT_DESKTOP:-}" in
    *XFCE*)  T=xfce4-terminal; ARGS="--title=AeroTop --geometry=120x40 -x" ;;
    *GNOME*) T=gnome-terminal; ARGS="--title=AeroTop --geometry=120x40 --" ;;
    *MATE*)  T=mate-terminal;  ARGS="--title=AeroTop --geometry=120x40 -x" ;;
    *)       T="" ;;
esac
if [ -n "$T" ] && command -v "$T" >/dev/null 2>&1; then
    sed -e "s|^Exec=.*|Exec=$T $ARGS $EXEC|" -e "s|^Terminal=.*|Terminal=false|" \
        "$TMP/aerotop.desktop" > "$APP_DIR/aerotop.desktop"
else
    sed -e "s|^Exec=.*|Exec=$EXEC|" "$TMP/aerotop.desktop" > "$APP_DIR/aerotop.desktop"
fi
chmod 644 "$APP_DIR/aerotop.desktop"
atualizar_menu

say "AeroTop instalado!"
echo "    Programa: $BIN_DIR/aerotop"
echo "    Menu:     Sistema → AeroTop"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo
       echo "    Atenção: $BIN_DIR não está no seu PATH. Adicione ao ~/.bashrc:"
       echo "      export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac
echo
echo "    Dica: rode 'aerotop --ram' uma vez para mostrar o tipo e a velocidade dos pentes."
