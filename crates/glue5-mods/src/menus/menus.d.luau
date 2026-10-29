
-- Mods' menus (`ui.menu`): the app's Mods page (MENUS), the Studio and MCP show them.

-- What every item has. `note` is the line under the label (descriptions).
export type ItemOpts = { note: string?, enabled: boolean?, visible: boolean? }

export type Item = {
	id: number,          -- read-only
	kind: string,        -- read-only: "button" | "toggle" | ... (as in the JSON)
	label: string,
	note: string?,
	enabled: boolean,    -- false: shown dimmed, not usable
	visible: boolean,    -- false: not shown at all
	remove: (self: any) -> (),
}

export type Button = Item & {
	action: string,      -- the pill's text ("Run")
	onPress: (() -> ())?,
	set: (self: Button, label: string) -> (),
}
export type Toggle = Item & { value: boolean, onChange: ((value: boolean) -> ())?, set: (self: Toggle, value: boolean) -> () }
export type Slider = Item & {
	value: number, min: number, max: number, step: number, unit: string?,
	onChange: ((value: number) -> ())?, set: (self: Slider, value: number) -> (),
}
export type Number = Item & {
	value: number, min: number?, max: number?, step: number, unit: string?,
	onChange: ((value: number) -> ())?, set: (self: Number, value: number) -> (),
}
export type Choice = Item & {
	value: number,               -- the option's index, from 1
	option: string,              -- read-only: options[value]
	options: { string },
	onChange: ((index: number, option: string) -> ())?,
	set: (self: Choice, value: number | string) -> (),  -- index or the option's text
}
export type Multi = Item & {
	value: { number },           -- the chosen indices, from 1, ascending
	options: { string },
	onChange: ((indices: { number }, options: { string }) -> ())?,
	set: (self: Multi, value: { number | string }) -> (),
}
export type Text = Item & {
	value: string, placeholder: string?, max: number,
	onChange: ((text: string) -> ())?, set: (self: Text, value: string) -> (),
}
export type ColorItem = Item & {
	value: Rgba, alpha: boolean,
	onChange: ((color: Rgba) -> ())?, set: (self: ColorItem, value: Color) -> (),  -- any Color
}
export type Label = Item & { set: (self: Label, text: string) -> () }
export type Separator = Item
export type Progress = Item & { value: number, set: (self: Progress, value: number) -> () }  -- 0..1

export type ButtonOpts = { note: string?, enabled: boolean?, visible: boolean?, action: string? }
export type SliderOpts = { note: string?, enabled: boolean?, visible: boolean?, step: number?, unit: string? }
export type NumberOpts = { note: string?, enabled: boolean?, visible: boolean?, min: number?, max: number?, step: number?, unit: string? }
export type TextOpts = { note: string?, enabled: boolean?, visible: boolean?, placeholder: string?, max: number? }
export type ColorOpts = { note: string?, enabled: boolean?, visible: boolean?, alpha: boolean? }
-- `search`: a field over the menu's rows filters them by what players type (label and note).
export type SubmenuOpts = { note: string?, enabled: boolean?, visible: boolean?, search: boolean? }

export type Menu = {
	id: number,                  -- read-only
	title: string,
	note: string?,               -- top-level: shown under the title in the Mods page's list
	search: boolean,             -- a field over its rows filters them as players type (long lists)
	enabled: boolean,            -- a submenu's row in its parent (no effect on a top-level menu)
	visible: boolean,            -- likewise
	-- A row with a pill (`action`, "Run" by default); `onPress` runs as a task.
	button: (self: Menu, label: string, opts: (string | ButtonOpts | () -> ())?, onPress: (() -> ())?) -> Button,
	-- A switch.
	toggle: (self: Menu, label: string, value: boolean?, opts: (string | ItemOpts | (value: boolean) -> ())?, onChange: ((value: boolean) -> ())?) -> Toggle,
	-- A value from min to max on steps of `step` ((max - min) / 100 by default; a number in the opts' place is the step).
	slider: (self: Menu, label: string, value: number, min: number, max: number,
		opts: (number | string | SliderOpts | (value: number) -> ())?, onChange: ((value: number) -> ())?) -> Slider,
	-- An exact number, stepped (step 1 by default) or typed; min and max are optional.
	number: (self: Menu, label: string, value: number,
		opts: (string | NumberOpts | (value: number) -> ())?, onChange: ((value: number) -> ())?) -> Number,
	-- One of the options (1 or more); `value` is its index from 1 or its text.
	choice: (self: Menu, label: string, options: { string }, value: (number | string)?,
		opts: (string | ItemOpts | (index: number, option: string) -> ())?, onChange: ((index: number, option: string) -> ())?) -> Choice,
	-- Any of the options (1 or more), by index from 1 or text.
	multi: (self: Menu, label: string, options: { string }, value: { number | string }?,
		opts: (string | ItemOpts | (indices: { number }, options: { string }) -> ())?, onChange: ((indices: { number }, options: { string }) -> ())?) -> Multi,
	-- One line of text (64 characters by default, 256 at most); `onChange` runs when the player commits it.
	text: (self: Menu, label: string, value: string?,
		opts: (string | TextOpts | (text: string) -> ())?, onChange: ((text: string) -> ())?) -> Text,
	-- A colour, white by default; opacity only with `alpha`.
	color: (self: Menu, label: string, value: Color?,
		opts: (string | ColorOpts | (color: Rgba) -> ())?, onChange: ((color: Rgba) -> ())?) -> ColorItem,
	-- Muted text, wrapped.
	label: (self: Menu, text: string, opts: ItemOpts?) -> Label,
	-- A section heading (none: a gap).
	separator: (self: Menu, title: string?) -> Separator,
	-- A bar from 0 to 1, with its percentage.
	progress: (self: Menu, label: string, value: number, opts: (string | ItemOpts)?) -> Progress,
	-- A menu inside this one (at any depth): a row that opens it. Returns the new menu.
	submenu: (self: Menu, label: string, opts: (string | SubmenuOpts)?) -> Menu,
	clear: (self: Menu) -> (),   -- removes every item (and the submenus they open)
	remove: (self: Menu) -> (),  -- removes the menu (a submenu's row too)
	show: (self: Menu) -> (),    -- asks the app to open the Mods page at this menu (once in 2 s)
	-- Run (as tasks) when a player starts / stops looking at the menu. nil clears.
	onOpen: (self: Menu, f: (() -> ())?) -> (),
	onClose: (self: Menu, f: (() -> ())?) -> (),
}

declare ui: {
	-- A menu the app shows (the Mods page's MENUS: F7, or the button beside the settings
	-- button on touch screens) and the Studio too; a mod with one stays running.
	-- Any number of menus, nested at any depth: 4096 menus and items a mod, all together.
	menu: (title: string, opts: (string | { note: string?, search: boolean? })?) -> Menu,
	-- A notification over the game.
	notify: (text: string) -> (),
}
