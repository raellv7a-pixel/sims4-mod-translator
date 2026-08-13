import os
import sys
from PyQt6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout, QLabel, QPushButton,
    QFileDialog, QTableWidget, QTableWidgetItem, QHeaderView, QComboBox,
    QProgressBar, QLineEdit, QSplitter, QStatusBar, QMessageBox, QFrame,
    QApplication
)
from PyQt6.QtCore import Qt, QThread, pyqtSignal
from PyQt6.QtGui import QColor

from core.dbpf import DBPFPackage
from core.stbl import STBLResource
from core.translator import TranslationEngine
from core.standalone import StandalonePackageGenerator

class TranslationWorker(QThread):
    progress_signal = pyqtSignal(int, int, int, str, str, str) # row_index, current, total, key_hex, translated_text, error
    finished_signal = pyqtSignal()
    error_signal = pyqtSignal(str)

    def __init__(self, items, engine_name, translator):
        super().__init__()
        self.items = items
        self.engine_name = engine_name
        self.translator = translator
        self.is_running = True

    def run(self):
        total = len(self.items)
        for i, item in enumerate(self.items):
            if not self.is_running:
                break
            key_hex = item["key_hex"]
            en_text = item["value"]
            
            try:
                if self.engine_name == "Argos Translate (Offline NMT)":
                    pt_text = self.translator.translate_argos(en_text, "en", "pt")
                elif self.engine_name == "Google Translate (Web API)":
                    pt_text = self.translator.translate_google(en_text, "en", "pt")
                elif self.engine_name == "Ollama (Local AI API)":
                    pt_text = self.translator.translate_ollama(en_text, "en", "pt")
                else:
                    pt_text = en_text
                if not self.translator.safeguard.preserves_tokens(en_text, pt_text):
                    raise ValueError("o motor alterou marcadores ou tags protegidos")
            except Exception as e:
                pt_text = en_text
                error = str(e)
            else:
                error = ""
                
            self.progress_signal.emit(i, i + 1, total, key_hex, pt_text, error)
            
        self.finished_signal.emit()

    def stop(self):
        self.is_running = False

class ArgosDownloadWorker(QThread):
    success_signal = pyqtSignal()
    error_signal = pyqtSignal(str)

    def __init__(self, translator):
        super().__init__()
        self.translator = translator

    def run(self):
        try:
            self.translator.install_argos_package("en", "pt")
            self.success_signal.emit()
        except Exception as e:
            self.error_signal.emit(str(e))

class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.translator = TranslationEngine()
        self.current_package_path = None
        self.current_package = None
        self.current_stbl = None
        self.entries = [] # list of dicts with key, key_hex, flags, original, translated
        self.worker = None
        self.dl_worker = None
        self.translation_errors = []
        
        self.init_ui()

    def init_ui(self):
        self.setWindowTitle("The Sims 4 Mod Translator for Linux (STBL PT-BR)")
        self.resize(1100, 700)
        self.apply_dark_theme()
        
        main_widget = QWidget()
        self.setCentralWidget(main_widget)
        layout = QVBoxLayout(main_widget)
        
        # Header / File Controls
        top_frame = QFrame()
        top_frame.setFrameShape(QFrame.Shape.StyledPanel)
        top_layout = QHBoxLayout(top_frame)
        
        self.lbl_file = QLabel("Nenhum arquivo .package carregado")
        self.lbl_file.setStyleSheet("font-weight: bold; font-size: 13px;")
        
        btn_open = QPushButton("📁 Abrir Mod (.package)")
        btn_open.clicked.connect(self.open_package)
        btn_open.setStyleSheet("padding: 6px 12px; background-color: #2b5b84; color: white; border-radius: 4px;")
        
        top_layout.addWidget(btn_open)
        top_layout.addWidget(self.lbl_file, 1)
        
        layout.addWidget(top_frame)
        
        # Translation Controls & Engine Selector
        engine_frame = QFrame()
        engine_frame.setFrameShape(QFrame.Shape.StyledPanel)
        engine_layout = QHBoxLayout(engine_frame)
        
        engine_layout.addWidget(QLabel("Motor de Tradução:"))
        self.combo_engine = QComboBox()
        self.combo_engine.addItems([
            "Argos Translate (Offline NMT)",
            "Google Translate (Web API)",
            "Ollama (Local AI API)",
            "Manual (Sem Tradução Automática)"
        ])
        engine_layout.addWidget(self.combo_engine)
        
        self.btn_download_argos = QPushButton("⬇️ Baixar Modelo EN->PT (Argos)")
        self.btn_download_argos.clicked.connect(self.download_argos_model)
        engine_layout.addWidget(self.btn_download_argos)
        
        self.btn_translate_all = QPushButton("⚡ Traduzir Todos")
        self.btn_translate_all.clicked.connect(self.start_translation)
        self.btn_translate_all.setStyleSheet("padding: 6px 12px; background-color: #1e7e34; color: white; border-radius: 4px; font-weight: bold;")
        engine_layout.addWidget(self.btn_translate_all)
        
        self.btn_export = QPushButton("💾 Exportar Tradução PT-BR (.package)")
        self.btn_export.clicked.connect(self.export_standalone_package)
        self.btn_export.setStyleSheet("padding: 6px 12px; background-color: #856404; color: white; border-radius: 4px; font-weight: bold;")
        engine_layout.addWidget(self.btn_export)
        
        layout.addWidget(engine_frame)
        
        # Search & Filter Bar
        search_layout = QHBoxLayout()
        search_layout.addWidget(QLabel("🔍 Pesquisar:"))
        self.txt_search = QLineEdit()
        self.txt_search.setPlaceholderText("Filtrar por texto ou Chave Hex (ex: 0xD4CC81A6)...")
        self.txt_search.textChanged.connect(self.filter_table)
        search_layout.addWidget(self.txt_search)
        layout.addLayout(search_layout)
        
        # Strings Table
        self.table = QTableWidget()
        self.table.setColumnCount(4)
        self.table.setHorizontalHeaderLabels(["ID / Chave Hex", "Texto Original (Inglês)", "Prévia de Tokens", "Tradução (Português PT-BR)"])
        self.table.horizontalHeader().setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        self.table.horizontalHeader().setSectionResizeMode(1, QHeaderView.ResizeMode.Interactive)
        self.table.horizontalHeader().setSectionResizeMode(2, QHeaderView.ResizeMode.Interactive)
        self.table.horizontalHeader().setSectionResizeMode(3, QHeaderView.ResizeMode.Stretch)
        self.table.itemChanged.connect(self.on_table_item_changed)
        layout.addWidget(self.table)
        
        # Progress Bar
        self.progress_bar = QProgressBar()
        self.progress_bar.setVisible(False)
        layout.addWidget(self.progress_bar)
        
        # Status Bar
        self.status_bar = QStatusBar()
        self.setStatusBar(self.status_bar)
        self.status_bar.showMessage("Pronto. Abra um arquivo .package para começar.")

    def apply_dark_theme(self):
        dark_stylesheet = """
        QMainWindow { background-color: #1e1e1e; color: #ffffff; }
        QWidget { background-color: #1e1e1e; color: #e0e0e0; font-family: 'Segoe UI', Ubuntu, sans-serif; }
        QFrame { border: 1px solid #333333; border-radius: 6px; background-color: #252526; }
        QTableWidget { gridline-color: #333333; background-color: #1e1e1e; color: #ffffff; border: 1px solid #333333; }
        QHeaderView::section { background-color: #2d2d30; color: #ffffff; padding: 6px; border: 1px solid #333333; font-weight: bold; }
        QTableWidget::item:selected { background-color: #094771; }
        QLineEdit, QComboBox { background-color: #3c3c3c; color: #ffffff; border: 1px solid #555555; padding: 5px; border-radius: 4px; }
        QPushButton { border: 1px solid #555555; border-radius: 4px; padding: 6px 12px; background-color: #3c3c3c; color: #ffffff; }
        QPushButton:hover { background-color: #505050; }
        QProgressBar { text-align: center; border: 1px solid #555555; border-radius: 4px; background-color: #252526; }
        QProgressBar::chunk { background-color: #007acc; }
        """
        self.setStyleSheet(dark_stylesheet)

    def open_package(self):
        file_path, _ = QFileDialog.getOpenFileName(self, "Selecionar Mod do The Sims 4", "", "Arquivos Package (*.package)")
        if not file_path:
            return
            
        try:
            pkg = DBPFPackage(file_path)
            if not pkg.stbl_entries:
                QMessageBox.warning(self, "Sem Tabelas de Texto", "Este arquivo .package não contém nenhuma tabela de texto (STBL).")
                return
                
            stbl_entry = pkg.stbl_entries[0]
            raw_stbl = pkg.read_payload(stbl_entry)
            stbl_res = STBLResource(raw_stbl)
            
            self.current_package_path = file_path
            self.current_package = pkg
            self.current_stbl = stbl_res
            
            self.lbl_file.setText(os.path.basename(file_path) + f" ({len(stbl_res.entries)} textos)")
            self.load_entries(stbl_res.entries)
            self.status_bar.showMessage(f"Carregados {len(stbl_res.entries)} textos. Clique em '⚡ Traduzir Todos' para traduzir automaticamente.")
        except Exception as e:
            QMessageBox.critical(self, "Erro ao Abrir Mod", f"Falha ao ler o arquivo .package:\n{str(e)}")

    def load_entries(self, stbl_entries):
        self.table.blockSignals(True)
        self.table.setRowCount(0)
        self.entries = []
        
        for item in stbl_entries:
            key_hex = item["key_hex"]
            en_val = item["value"]
            masked_val, tokens = self.translator.safeguard.mask(en_val)
            
            entry = {
                "key": item["key"],
                "key_hex": key_hex,
                "flags": item["flags"],
                "en_value": en_val,
                "pt_value": en_val
            }
            self.entries.append(entry)
            
            row = self.table.rowCount()
            self.table.insertRow(row)
            
            item_key = QTableWidgetItem(key_hex)
            item_key.setFlags(item_key.flags() ^ Qt.ItemFlag.ItemIsEditable)
            
            item_en = QTableWidgetItem(en_val)
            item_en.setFlags(item_en.flags() ^ Qt.ItemFlag.ItemIsEditable)
            
            item_masked = QTableWidgetItem(masked_val)
            item_masked.setFlags(item_masked.flags() ^ Qt.ItemFlag.ItemIsEditable)
            item_masked.setForeground(QColor("#808080"))
            
            item_pt = QTableWidgetItem(en_val)
            
            self.table.setItem(row, 0, item_key)
            self.table.setItem(row, 1, item_en)
            self.table.setItem(row, 2, item_masked)
            self.table.setItem(row, 3, item_pt)
            
        self.table.blockSignals(False)

    def on_table_item_changed(self, item):
        if item.column() == 3:
            row = item.row()
            if row < len(self.entries):
                self.entries[row]["pt_value"] = item.text()

    def filter_table(self, query):
        q = query.lower().strip()
        for row in range(self.table.rowCount()):
            key_text = self.table.item(row, 0).text().lower()
            en_text = self.table.item(row, 1).text().lower()
            pt_text = self.table.item(row, 3).text().lower()
            
            match = not q or (q in key_text or q in en_text or q in pt_text)
            self.table.setRowHidden(row, not match)

    def download_argos_model(self):
        if not self.translator.has_argos:
            QMessageBox.warning(self, "Argos Translate Indisponível", "A biblioteca argostranslate não foi detectada no ambiente Python.")
            return
            
        self.status_bar.showMessage("Baixando modelo de tradução Argos EN -> PT (Aguarde)...")
        self.btn_download_argos.setEnabled(False)
        
        self.dl_worker = ArgosDownloadWorker(self.translator)
        self.dl_worker.success_signal.connect(self.on_argos_dl_success)
        self.dl_worker.error_signal.connect(self.on_argos_dl_error)
        self.dl_worker.start()

    def on_argos_dl_success(self):
        self.btn_download_argos.setEnabled(True)
        self.status_bar.showMessage("Modelo Argos EN -> PT instalado com sucesso!")
        QMessageBox.information(self, "Argos Translate", "Modelo offline Inglês -> Português baixado e pronto para uso!")

    def on_argos_dl_error(self, err):
        self.btn_download_argos.setEnabled(True)
        self.status_bar.showMessage("Erro ao baixar modelo Argos.")
        QMessageBox.critical(self, "Erro Argos Translate", f"Não foi possível baixar o modelo:\n{err}")

    def start_translation(self):
        if not self.entries:
            QMessageBox.warning(self, "Sem Dados", "Carregue um arquivo .package primeiro.")
            return
            
        engine_name = self.combo_engine.currentText()
        if "Argos" in engine_name and not self.translator.get_argos_languages():
            QMessageBox.warning(self, "Modelo Não Instalado", "Instale o modelo Argos EN->PT clicando no botão 'Baixar Modelo EN->PT'.")
            return
            
        untranslated_items = [{"key_hex": e["key_hex"], "value": e["en_value"]} for e in self.entries]
        
        self.btn_translate_all.setEnabled(False)
        self.progress_bar.setValue(0)
        self.progress_bar.setVisible(True)
        self.status_bar.showMessage("Traduzindo textos...")
        self.translation_errors = []
        
        self.worker = TranslationWorker(untranslated_items, engine_name, self.translator)
        self.worker.progress_signal.connect(self.on_translation_progress)
        self.worker.finished_signal.connect(self.on_translation_finished)
        self.worker.start()

    def on_translation_progress(self, row_idx, current, total, key_hex, translated_text, error):
        percent = int((current / total) * 100)
        self.progress_bar.setValue(percent)
        self.status_bar.showMessage(f"Traduzindo [{current}/{total}] ({key_hex}): {translated_text[:45]}...")
        
        self.table.blockSignals(True)
        if row_idx < len(self.entries):
            self.entries[row_idx]["pt_value"] = translated_text
            
            item_pt = QTableWidgetItem(translated_text)
            if error:
                self.translation_errors.append((key_hex, error))
                item_pt.setBackground(QColor("#5a251f"))
            else:
                item_pt.setBackground(QColor("#1e4620"))
            self.table.setItem(row_idx, 3, item_pt)
            
        self.table.blockSignals(False)

    def on_translation_finished(self):
        self.btn_translate_all.setEnabled(True)
        self.progress_bar.setVisible(False)
        if self.translation_errors:
            failed_count = len(self.translation_errors)
            self.status_bar.showMessage(f"Tradução concluída com {failed_count} texto(s) restaurado(s) ao original.")
            QMessageBox.warning(
                self,
                "Tradução Concluída com Avisos",
                f"{failed_count} texto(s) tiveram marcadores do jogo alterados pelo motor e foram restaurados ao inglês. "
                "As linhas em vermelho podem ser traduzidas manualmente, preservando os marcadores.")
        else:
            self.status_bar.showMessage("Tradução de todos os textos concluída!")
            QMessageBox.information(self, "Tradução Concluída", "Todos os textos foram traduzidos! Você pode ajustar os textos manualmente se desejar antes de exportar.")

    def resolve_stbl_instance(self):
        if not self.current_package or not self.current_package.stbl_entries:
            return 0x0007401A, 0x6778F312, 0x00000000

        orig_stbl = self.current_package.stbl_entries[0]
        base_high = orig_stbl["inst_h"]
        base_low = orig_stbl["inst_l"]
        group_id = orig_stbl["group"]

        # If a template file is loaded, try matching the main mod .package file in the same directory
        if self.current_package_path:
            dir_name, file_name = os.path.split(self.current_package_path)
            if "TEMPLATE" in file_name.upper():
                possible_names = [
                    file_name.replace("_Translation_TEMPLATE.package", ".package"),
                    file_name.replace("_TEMPLATE.package", ".package"),
                    file_name.replace("TEMPLATE.package", ".package"),
                    file_name.replace("_Translation_TEMPLATE", "").replace("_TEMPLATE", "")
                ]
                
                for candidate in possible_names:
                    candidate_path = os.path.join(dir_name, candidate)
                    if candidate_path != self.current_package_path and os.path.exists(candidate_path) and candidate_path.endswith(".package"):
                        try:
                            main_pkg = DBPFPackage(candidate_path)
                            if main_pkg.stbl_entries:
                                main_stbl = main_pkg.stbl_entries[0]
                                return main_stbl["inst_h"], main_stbl["inst_l"], main_stbl["group"]
                        except Exception:
                            pass

        return base_high, base_low, group_id

    def export_standalone_package(self):
        if not self.entries:
            QMessageBox.warning(self, "Sem Dados", "Nenhuma tradução disponível para exportar.")
            return

        validation_error = self.validate_translation_entries()
        if validation_error:
            QMessageBox.critical(self, "Tradução Inválida", validation_error)
            return
            
        save_path, _ = QFileDialog.getSaveFileName(
            self,
            "Salvar Pacote de Tradução Standalone PT-BR",
            "Mod_PTBR_Translation.package",
            "Arquivos Package (*.package)"
        )
        if not save_path:
            return
            
        try:
            stbl_entries = []
            for e in self.entries:
                stbl_entries.append({
                    "key": e["key"],
                    "flags": e["flags"],
                    "value": e["pt_value"]
                })
                
            stbl_res = STBLResource()
            stbl_res.entries = stbl_entries
            uncompressed_bytes = stbl_res.compile()
            
            base_high, base_low, group_id = self.resolve_stbl_instance()
                
            StandalonePackageGenerator.generate(
                uncompressed_bytes,
                base_high=base_high,
                base_low=base_low,
                group_id=group_id,
                output_path=save_path
            )
            
            self.status_bar.showMessage(f"Pacote salvo em: {save_path}")
            QMessageBox.information(
                self,
                "Pacote Standalone Salvo",
                f"O arquivo de tradução foi criado com sucesso:\n{save_path}\n\nColoque este arquivo na sua pasta Mods junto com o mod original!"
            )
        except Exception as e:
            QMessageBox.critical(self, "Erro ao Salvar Pacote", f"Falha ao gerar o arquivo .package:\n{str(e)}")

    def validate_translation_entries(self):
        for entry in self.entries:
            translated = entry["pt_value"]
            if translated.startswith("[ERRO: "):
                return f"A chave {entry['key_hex']} contém um erro de tradução. Corrija-a antes de exportar."
            if "\x00" in translated:
                return f"A chave {entry['key_hex']} contém um caractere NUL, que o jogo não aceita."

            if not self.translator.safeguard.preserves_tokens(entry["en_value"], translated):
                return (
                    f"A chave {entry['key_hex']} alterou ou removeu marcadores do jogo "
                    "(por exemplo, {{0.String}} ou tags <font>)."
                )

        return None
