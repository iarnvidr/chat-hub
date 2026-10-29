from pathlib import Path


ROOT = Path(__file__).parents[1]


def test_desktop_uses_tauri_runtime_as_sole_tokio_owner():
    source = (ROOT / "src-tauri/src/main.rs").read_text()
    assert "#[tokio::main]" not in source
    assert "fn main() {" in source
    # These synchronous bootstrap waits are safe only before Tauri's event loop
    # invokes setup from a runtime context. Keep the runtime owner architecture.
    assert source.count("tauri::async_runtime::block_on(") == 2
    assert source.index("fn main() {") < source.index(".setup(|app|")


def test_exception_stack_array_is_rendered_as_description_before_utf8():
    source = (ROOT / "src-tauri/src/crash_diagnostics.rs").read_text()
    assert "unsafe fn send_object(object: Id, selector: &[u8]) -> Id" in source
    assert "unsafe fn string_utf8(value: Id) -> String" in source
    assert 'let stack_array = send_object(exception, b"callStackSymbols\\0");' in source
    assert 'let stack_description = send_object(stack_array, b"description\\0");' in source
    assert "let stack = string_utf8(stack_description);" in source
    assert 'let name = string_utf8(send_object(exception, b"name\\0"));' in source
    assert 'let reason = string_utf8(send_object(exception, b"reason\\0"));' in source
    assert source.index('write_all(initial.as_bytes())') < source.index('b"callStackSymbols\\0"')
    assert 'catch_unwind(std::panic::AssertUnwindSafe' in source
