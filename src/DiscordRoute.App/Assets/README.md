# Иконка «Связь»

Создана встроенным image_gen. Итоговый исходник: icon-source.png.
app.ico — упаковка того же изображения в размерах 16/24/32/48/64/128/256 для Windows;
размеры подготовлены sips. Перед упаковкой все PNG-кадры преобразованы в RGBA
(8 бит на канал, 32 бита на пиксель), как требуется для PNG внутри Windows ICO.
RGB-пиксели сохранены без изменений, добавлен непрозрачный альфа-канал.
Проверка IconChecks не позволяет выпустить 24-битные RGB-кадры в таком ICO.

Итоговый промпт:

A Windows app icon. Square canvas, fully opaque, the entire canvas filled edge to
edge with ONE uniform solid teal color #0F766E. Centered is a bold simple white
chain link symbol: two interlocking diagonal links. Pure flat vector-like two-color
graphic. White symbol occupies central 65 percent of canvas. Perfectly clean
background of one solid teal color everywhere, zero black areas. No gradients,
shadows, textures, lighting, transparency, holes in background, rounded background
tile, frames, text, letters, mockup or watermark. Minimalist icon readable at 16px,
all shapes smooth, white symbol on continuous solid teal background.
