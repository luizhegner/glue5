
-- A colour, as menus and draw both take it: 0xRRGGBBAA (draw.rgba makes one), {r, g, b, a} with
-- 0-255 each (a 255 when left out), or "#rrggbb" / "#rrggbbaa".
export type Color = number | Rgba | string
-- A colour's parts, 0-255 each (`a` 255 when left out): what menus give back.
export type Rgba = { r: number, g: number, b: number, a: number? }
