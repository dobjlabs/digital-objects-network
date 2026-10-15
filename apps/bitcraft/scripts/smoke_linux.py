"""Exercise window and tray lifecycle in a private D-Bus/X11 test session."""

import argparse
import ctypes
from pathlib import Path
import subprocess
import tempfile
import threading
import time

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
watcher_iface = 'org.kde.StatusNotifierWatcher'
registered = []

class Watcher(dbus.service.Object):
    @dbus.service.method(watcher_iface, in_signature='s', out_signature='', sender_keyword='sender')
    def RegisterStatusNotifierItem(self, service, sender):
        service = str(service)
        registered.append((str(sender), service) if service.startswith('/') else (service, '/StatusNotifierItem'))
        self.StatusNotifierItemRegistered(service)

    @dbus.service.signal(watcher_iface, signature='s')
    def StatusNotifierItemRegistered(self, service):
        pass

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='ss', out_signature='v')
    def Get(self, interface, name):
        return self.GetAll(interface)[name]

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='s', out_signature='a{sv}')
    def GetAll(self, interface):
        return {'IsStatusNotifierHostRegistered': dbus.Boolean(True),
                'ProtocolVersion': dbus.Int32(0),
                'RegisteredStatusNotifierItems': dbus.Array([service + path for service, path in registered], signature='s')}

name = dbus.service.BusName(watcher_iface, bus=bus)
watcher = Watcher(name, '/StatusNotifierWatcher')
loop = GLib.MainLoop()
thread = threading.Thread(target=loop.run, daemon=True)
thread.start()

xlib = ctypes.CDLL('libX11.so.6')
xlib.XOpenDisplay.restype = ctypes.c_void_p
xlib.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
xlib.XInternAtom.restype = ctypes.c_ulong
xlib.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
xlib.XFlush.argtypes = [ctypes.c_void_p]
xlib.XCloseDisplay.argtypes = [ctypes.c_void_p]

class ClientData(ctypes.Union):
    _fields_ = [('b', ctypes.c_char * 20), ('s', ctypes.c_short * 10), ('l', ctypes.c_long * 5)]
class ClientEvent(ctypes.Structure):
    _fields_ = [('type', ctypes.c_int), ('serial', ctypes.c_ulong), ('send_event', ctypes.c_int),
                ('display', ctypes.c_void_p), ('window', ctypes.c_ulong), ('message_type', ctypes.c_ulong),
                ('format', ctypes.c_int), ('data', ClientData)]
class Event(ctypes.Union):
    _fields_ = [('client', ClientEvent), ('pad', ctypes.c_long * 24)]

def close_window(window):
    display = xlib.XOpenDisplay(None)
    event = Event()
    event.client.type = 33
    event.client.display = display
    event.client.window = window
    event.client.message_type = xlib.XInternAtom(display, b'WM_PROTOCOLS', 0)
    event.client.format = 32
    event.client.data.l[0] = xlib.XInternAtom(display, b'WM_DELETE_WINDOW', 0)
    xlib.XSendEvent(display, window, 0, 0, ctypes.byref(event))
    xlib.XFlush(display)
    xlib.XCloseDisplay(display)

def wait_for(predicate, label, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = predicate()
        if result:
            print('PASS:', label, flush=True)
            return result
        time.sleep(0.1)
    raise AssertionError(label)

def find_window(pid):
    result = subprocess.run(['xdotool', 'search', '--pid', str(pid), '--name', '^Bitcraft$'], capture_output=True, text=True)
    if result.returncode == 0:
        for candidate in result.stdout.splitlines():
            properties = subprocess.run(['xprop', '-id', candidate, '_NET_WM_WINDOW_TYPE'], capture_output=True, text=True)
            if '_NET_WM_WINDOW_TYPE_NORMAL' in properties.stdout:
                return int(candidate)
    return None

def visible(window):
    result = subprocess.run(['xwininfo', '-id', str(window)], capture_output=True, text=True)
    return 'Map State: IsViewable' in result.stdout

def find_menu(layout, label):
    if str(layout[1].get('label', '')) == label:
        return int(layout[0])
    for child in layout[2]:
        result = find_menu(child, label)
        if result is not None:
            return result
    return None

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', type=Path, default=Path(__file__).resolve().parents[1] / 'src-tauri/target/release/bitcraft-desktop')
binary = parser.parse_args().binary.resolve()

with tempfile.TemporaryDirectory(prefix='bitcraft-desktop-smoke-') as temporary:
    root = Path(temporary) / 'data'
    log_path = Path(temporary) / 'app.log'
    processes = []
    try:
        with log_path.open('w') as log:
            app = subprocess.Popen([str(binary), '--no-runtime', '--background', '--home', str(root)], stdout=log, stderr=log)
            processes.append(app)
            window = wait_for(lambda: find_window(app.pid), 'game window created')
            wait_for(lambda: registered, 'tray registered with simulated StatusNotifier host')
            wait_for(lambda: not visible(window) and app.poll() is None, 'background startup hides the window')
            service, item_path = registered[-1]
            item = bus.get_object(service, item_path)
            props = dbus.Interface(item, 'org.freedesktop.DBus.Properties')
            menu_path = str(props.Get('org.kde.StatusNotifierItem', 'Menu'))
            menu = dbus.Interface(bus.get_object(service, menu_path), 'com.canonical.dbusmenu')
            _, layout = menu.GetLayout(0, -1, dbus.Array([], signature='s'))
            manage = find_menu(layout, 'Open Bitcraft')
            quit_id = find_menu(layout, 'Quit Bitcraft')
            assert manage is not None and quit_id is not None
            menu.Event(manage, 'clicked', dbus.Int32(0, variant_level=1), dbus.UInt32(0))
            wait_for(lambda: visible(window), 'tray Open restores hidden window')
            close_window(window)
            wait_for(lambda: not visible(window) and app.poll() is None, 'window close hides to tray')
            second = subprocess.run([str(binary), '--no-runtime', '--home', str(root)], timeout=10)
            assert second.returncode == 0
            wait_for(lambda: visible(window), 'second launch restores the existing window')
            close_window(window)
            wait_for(lambda: not visible(window), 'second close hides the restored window')
            bus.release_name(watcher_iface)
            wait_for(lambda: visible(window) and app.poll() is None, 'losing tray host restores the window')
            menu.Event(quit_id, 'clicked', dbus.Int32(0, variant_level=1), dbus.UInt32(0))
            wait_for(lambda: app.poll() is not None, 'tray Quit exits the app')
            assert app.returncode == 0
            app = subprocess.Popen([str(binary), '--no-runtime', '--background', '--home', str(root)], stdout=log, stderr=log)
            processes.append(app)
            window = wait_for(lambda: find_window(app.pid), 'window created without a tray host')
            time.sleep(3)
            assert visible(window) and app.poll() is None, f'visible={visible(window)} exit={app.poll()} watcher={bus.name_has_owner(watcher_iface)}'
            print('PASS: background startup stays visible without a host', flush=True)
            close_window(window)
            wait_for(lambda: app.poll() is not None, 'window close exits without a host')
            assert app.returncode == 0
            assert not root.exists()
            print('PASS: no installation data was created', flush=True)
    except Exception:
        print(log_path.read_text(), flush=True)
        raise
    finally:
        for app in processes:
            if app.poll() is None:
                app.terminate()
                app.wait(timeout=5)
        loop.quit()
