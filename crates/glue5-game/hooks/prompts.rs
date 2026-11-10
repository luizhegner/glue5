// Hooks for the runtime's `prompts.rs` (see ../hooks.rs).
&[
    // Every Scaleform method call (the scripts' and the code's menus), its
    // arguments as GFxValues: the instructional buttons' bars are read here.
    ("scaleform_call_method", "CScaleformMgr::CallMethodInternal(int, int, eSCALEFORM_BASE_CLASS, char const*, GFxValue const*, int, bool, bool, int, int)"),
    // Scripts asking whether an input was just pressed: a scene waiting for
    // its skip.
    ("is_control_just_pressed", "pad_commands::SetupScriptCommands()::scrWrapped_IS_CONTROL_JUST_PRESSED::Call(rage::scrThread::Info&)"),
    ("is_disabled_control_just_pressed", "pad_commands::SetupScriptCommands()::scrWrapped_IS_DISABLED_CONTROL_JUST_PRESSED::Call(rage::scrThread::Info&)"),
    ("control_instructional_button", "pad_commands::SetupScriptCommands()::scrWrapped_GET_CONTROL_INSTRUCTIONAL_BUTTON::Call(rage::scrThread::Info&)"),
    // Called: an input by its name (the help text's ~INPUT_X~), where it is
    // on the pad, pressing it (as the game's mouse clicks on the bar do), a
    // warning screen up.
    ("control_input_by_name", "CControl::GetInputByName(char const*)"),
    ("control_input_source", "CControl::GetInputSource(rage::InputType, int, unsigned char, bool) const"),
    ("io_value_set_next", "rage::ioValue::SetNextValue(float, rage::ioSource const&)"),
    ("warning_screen_active", "CWarningScreen::IsActive()"),
]
