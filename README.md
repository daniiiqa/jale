# jale

*Jale*: "¿le jalas?". El launcher de discada: las apps y todos los menús del
rice (ajustes, looks, portapapeles, emoji, calculadora, atajos, wallpapers,
ventanas), con el theme de cada look. Toma el lugar de rofi y habla su idioma:
lee los mismos `.rasi` y entiende lo que los scripts le mandaban a `rofi
-dmenu`, así que cada look trae su menú sin copiar nada.

```
jale                          las apps (lo que más usas, arriba)
jale -show window             las ventanas abiertas
jale -dmenu [opciones]        un menú con lo que llegue por stdin
jale -dump-fonts [-config f]  las fuentes del theme (para FontSize.py)
```

En `-dmenu`: `-p`, `-mesg`, `-format s|i`, `-no-custom`, `-markup-rows`,
`-l`, `-selected-row`, `-a`/`-u` (filas active y urgent), `-kb-custom-N`
(sale con 9+N, como rofi), `-config` y `-theme-str`. Cada renglón puede traer
su ícono como en rofi: `texto\0icon\x1fnombre-o-ruta`. Sale con 1 si se
cancela. Una segunda llamada cierra la que esté abierta.

El theme: `~/.config/jale/config.rasi` (`menu.rasi` en `-dmenu`) o el
`-config` de cada script, con sus `@import`, `@theme`, variables, `inherit`,
colores, medidas (px, em, %), bordes por lado, radios, imagen de fondo, filas y
columnas (`children`, `orientation`, `expand`) y la lista en cuadrícula. Se
dibuja con las fuentes del theme (cosmic-text) sobre layer-shell.

Teclado: escribir filtra (búsqueda difusa), ↑↓ Tab Ctrl+J/K eligen (←→ en
cuadrícula), Enter abre, Shift+Enter manda lo escrito, Ctrl+U y Ctrl+W
borran, Esc cierra. Mouse: pasar elige, clic abre, la rueda baja y sube.

```
cargo install --git https://github.com/daniiiqa/jale --root ~/.local --locked
```

MIT.
