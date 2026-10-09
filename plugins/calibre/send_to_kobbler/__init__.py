from calibre.customize import InterfaceActionBase


class SendToKobblerPlugin(InterfaceActionBase):
    name = 'Send to Kobbler'
    description = 'Upload KEPUB and metadata to Kobbler'
    supported_platforms = ['windows', 'osx', 'linux']
    author = 'Kobbler'
    version = (0, 3, 0)
    minimum_calibre_version = (8, 0, 0)

    actual_plugin = 'calibre_plugins.send_to_kobbler.kobbler:SendToKobblerAction'

    def is_customizable(self):
        return True

    def config_widget(self):
        from calibre_plugins.send_to_kobbler.kobbler import ConfigWidget

        return ConfigWidget()

    def save_settings(self, config_widget):
        config_widget.save_settings()
