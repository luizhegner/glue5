--!strict
-- Draw: an image of the mod's own, turning, with a caption over a panel.

local logo = draw.loadImage("images/logo.png")
local width, height = draw.screenSize()
print("logo", logo.width, logo.height, "on a screen of", width, height)

-- Made once, passed with each call; a field changes as needed.
local turning = draw.style({ z = 1 })
local panel = draw.style({ rounding = 0.01 })
local caption = draw.style({ align = "center", z = 2 })
local turn = 0
while true do
	turn += 1
	draw.rect(0.5, 0.86, 0.6, 0.1, 0x000000aa, panel)
	-- No height: the image's own proportions.
	draw.image(logo, 0.2, 0.35, 0.2)
	turning.rotation = turn
	draw.image(logo, 0.8, 0.35, 0.15, 0.3, 0xffffff8c, turning)
	draw.text("GLUE 5 draw test", 0.5, 0.83, 0.05, 0xffdc00ff, caption)
	wait()
end
