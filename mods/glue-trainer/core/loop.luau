--!strict
-- What runs every frame: the features that are on add a tick here and take it away when
-- switched off. One loop runs them all; a tick that fails is taken away (its feature says
-- why), the others go on.

type Tick = {
	name: string,
	run: () -> (),
	-- Called (once) when the tick failed.
	failed: ((message: string) -> ())?,
}

local ticks: { Tick } = {}

local M = {}

-- `run` every frame from now on, under `name` (one tick per name: a second replaces it).
function M.add(name: string, run: () -> (), failed: ((message: string) -> ())?)
	M.remove(name)
	table.insert(ticks, { name = name, run = run, failed = failed })
end

function M.remove(name: string)
	for i, t in ticks do
		if t.name == name then
			table.remove(ticks, i)
			return
		end
	end
end

function M.has(name: string): boolean
	for _, t in ticks do
		if t.name == name then
			return true
		end
	end
	return false
end

-- The loop, as a task of its own.
function M.start()
	task.spawn(function()
		while true do
			-- A copy: a tick may add or remove ticks.
			for _, t in table.clone(ticks) do
				local ok, err = pcall(t.run :: any)
				if not ok then
					M.remove(t.name)
					local message = tostring(err)
					print(`{t.name} stopped: {message}`)
					if t.failed then
						t.failed(message)
					end
				end
			end
			wait()
		end
	end)
end

return M
