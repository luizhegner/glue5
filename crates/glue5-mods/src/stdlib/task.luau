-- `task.every(ms, f, ...)`: `f` every `ms` milliseconds of game time, as a
-- task, until the function it returns is called (or `f` fails).

local spawn, wait = task.spawn, task.wait

task.every = function(ms: number, f: (...any) -> ...any, ...: any): () -> ()
	if type(ms) ~= "number" or ms ~= ms then
		error("task.every: the interval is a number of milliseconds", 2)
	end
	if type(f) ~= "function" then
		error("task.every: the second argument is a function", 2)
	end
	local args = table.pack(...)
	local running = true
	spawn(function()
		while running do
			wait(ms)
			if running then
				f(table.unpack(args, 1, args.n))
			end
		end
	end)
	return function()
		running = false
	end
end
