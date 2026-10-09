import json
import os
import traceback
import uuid
from functools import partial
from urllib import request

from calibre.gui2 import error_dialog, info_dialog
from calibre.gui2.actions import InterfaceAction
from calibre.gui2.threaded_jobs import ThreadedJob
from calibre.utils.config import JSONConfig

try:
    from qt.core import (
        QHBoxLayout,
        QLineEdit,
        QMenu,
        QMessageBox,
        QPushButton,
        QTimer,
        QVBoxLayout,
        QWidget,
    )
except Exception:
    QWidget = None
    QMenu = None
    QTimer = None

PREFS = JSONConfig('plugins/send_to_kobbler')
PREFS.defaults['base_url'] = 'http://localhost:6766'
PREFS.defaults['ingest_token'] = ''


def run_upload_job(book_data_list, base_url, ingest_token, log, abort, notifications):
    """
    Worker function for the upload job. Runs in a background thread.
    """
    failures = []
    total = len(book_data_list)
    synced_book_ids = []

    for i, book_data in enumerate(book_data_list):
        if abort.is_set():
            log('Job aborted')
            break

        book_id = book_data['calibre_id']
        title = book_data['title']
        notifications.put((i / total, f'Uploading: {title}'))
        log(f'Processing book {i + 1}/{total}: {title}')

        try:
            upload_single_book(book_data, base_url, ingest_token, log)
            synced_book_ids.append(book_id)
            log(f'Successfully uploaded: {title}')
        except Exception as exc:
            log(f'Failed to upload {title}: {exc}')
            log(traceback.format_exc())
            failures.append((book_id, title, str(exc)))

    notifications.put((1.0, f'Completed {total - len(failures)}/{total} books'))
    return {'failures': failures, 'synced_book_ids': synced_book_ids}


def upload_single_book(book_data, base_url, ingest_token, log):
    """Upload a single book to Kobbler using atomic ingest endpoint."""
    book_uuid = book_data['book_uuid']
    kepub_path = book_data['kepub_path']

    try:
        # Build metadata JSON
        metadata = {
            'title': book_data['title'],
            'description': book_data['description'],
            'language': book_data['language'],
            'publisher': book_data['publisher'],
            'pubdate': book_data['pubdate'],
            'series_name': book_data['series_name'],
            'series_index': book_data['series_index'],
            'authors': book_data['authors'],
        }

        log(f'  Uploading {book_uuid} (atomic ingest)')
        atomic_ingest(
            book_uuid, metadata, book_data.get('cover_bytes'), kepub_path, base_url, ingest_token
        )
        log(f'  Successfully uploaded {book_uuid}')
    finally:
        # Clean up temp KEPUB file if needed
        if book_data.get('cleanup_kepub') and os.path.exists(kepub_path):
            try:
                os.remove(kepub_path)
            except Exception:
                pass


def atomic_ingest(book_uuid, metadata, cover_bytes, file_path, base_url, ingest_token):
    """Upload a book using the atomic ingest endpoint (single multipart request)."""
    boundary = '----KobblerIngest' + str(uuid.uuid4()).replace('-', '')
    url = base_url.rstrip('/') + f'/api/v1/books/{book_uuid}/ingest'

    # Build multipart body manually
    body_parts = []

    # Add metadata JSON part
    body_parts.append(f'--{boundary}\r\n'.encode())
    body_parts.append(b'Content-Disposition: form-data; name="metadata"\r\n')
    body_parts.append(b'Content-Type: application/json\r\n\r\n')
    body_parts.append(json.dumps(metadata).encode())
    body_parts.append(b'\r\n')

    # Add format field
    body_parts.append(f'--{boundary}\r\n'.encode())
    body_parts.append(b'Content-Disposition: form-data; name="format"\r\n\r\n')
    body_parts.append(b'KEPUB\r\n')

    # Add cover if present
    if cover_bytes:
        body_parts.append(f'--{boundary}\r\n'.encode())
        body_parts.append(b'Content-Disposition: form-data; name="cover"; filename="cover.jpg"\r\n')
        body_parts.append(b'Content-Type: image/jpeg\r\n\r\n')
        body_parts.append(cover_bytes)
        body_parts.append(b'\r\n')

    # Add book file
    with open(file_path, 'rb') as f:
        file_data = f.read()
    filename = os.path.basename(file_path)
    body_parts.append(f'--{boundary}\r\n'.encode())
    body_parts.append(
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'.encode()
    )
    body_parts.append(b'Content-Type: application/octet-stream\r\n\r\n')
    body_parts.append(file_data)
    body_parts.append(b'\r\n')

    # End boundary
    body_parts.append(f'--{boundary}--\r\n'.encode())

    # Combine all parts
    body = b''.join(body_parts)

    # Make request
    req = request.Request(url, data=body, method='PUT')
    req.add_header('Content-Type', f'multipart/form-data; boundary={boundary}')
    req.add_header('Authorization', f'Bearer {ingest_token}')

    with request.urlopen(req) as resp:
        if resp.status >= 400:
            raise Exception(f'HTTP {resp.status} for atomic ingest')


def api_call(method, path, payload, content_type, base_url, ingest_token, raw=False):
    """Make an API call to Kobbler."""
    url = base_url.rstrip('/') + path
    if raw:
        body = payload
    else:
        body = json.dumps(payload).encode('utf-8') if payload is not None else None
    req = request.Request(url, data=body, method=method)
    req.add_header('Content-Type', content_type)
    req.add_header('Authorization', f'Bearer {ingest_token}')
    with request.urlopen(req) as resp:
        if resp.status >= 400:
            raise Exception(f'HTTP {resp.status} for {path}')


class ConfigWidget(QWidget):
    def __init__(self):
        super().__init__()
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)

        # URL field
        layout.addWidget(self._label('Kobbler Base URL'))
        url_row = QHBoxLayout()
        self.base_url = QLineEdit(PREFS['base_url'])
        self.test_button = QPushButton('Test')
        self.test_button.clicked.connect(self.test_connection)
        self.test_button.setFixedWidth(60)
        url_row.addWidget(self.base_url, 1)
        url_row.addWidget(self.test_button)
        layout.addLayout(url_row)

        # Token field
        layout.addWidget(self._label('Ingest Token'))
        self.ingest_token = QLineEdit(PREFS['ingest_token'])
        self.ingest_token.setEchoMode(QLineEdit.Password)
        layout.addWidget(self.ingest_token)

        layout.addStretch(1)

    def _label(self, text):
        from qt.core import QLabel

        label = QLabel(text)
        return label

    def save_settings(self):
        PREFS['base_url'] = self.base_url.text().strip()
        PREFS['ingest_token'] = self.ingest_token.text().strip()

    def test_connection(self):
        base_url = self.base_url.text().strip()
        if not base_url:
            QMessageBox.warning(self, 'Send to Kobbler', 'Enter a base URL first.')
            return
        url = base_url.rstrip('/') + '/health'
        try:
            req = request.Request(url, method='GET')
            with request.urlopen(req, timeout=5) as resp:
                body = resp.read(128).decode('utf-8', errors='ignore')
                if resp.status >= 400:
                    QMessageBox.critical(self, 'Send to Kobbler', f'HTTP {resp.status} from {url}')
                    return
                QMessageBox.information(self, 'Send to Kobbler', f'OK: {resp.status} {body}')
        except Exception as exc:
            QMessageBox.critical(self, 'Send to Kobbler', f'Failed to reach {url}:\n{exc}')


class SendToKobblerAction(InterfaceAction):
    name = 'Send to Kobbler'
    action_spec = ('Send to Kobbler', 'images/kobbler.png', 'Upload to Kobbler', None)
    action_add_menu = True
    action_menu_clone_qaction = True

    def genesis(self):
        self.menu = QMenu(self.gui)
        self.qaction.setMenu(self.menu)
        self.qaction.triggered.connect(self.send_selected)
        self.menu.aboutToShow.connect(self.build_menu)
        # Track pending conversion batches: batch_id -> {ready_books, pending_jobs, converted_book_ids}
        self._pending_batches = {}
        # Map job -> batch_id for conversion callback
        self._job_to_batch = {}

    def build_menu(self):
        """Build the menu dynamically based on current selection."""
        self.menu.clear()

        book_ids = list(self.gui.library_view.get_selected_ids())
        if not book_ids:
            action = self.menu.addAction('No books selected')
            action.setEnabled(False)
            return

        db = self.gui.current_db.new_api
        synced_count = 0
        total = len(book_ids)

        for book_id in book_ids:
            identifiers = db.field_for('identifiers', book_id) or {}
            if 'kobbler' in identifiers:
                synced_count += 1

        # Main send action with sync status
        if synced_count == 0:
            label = f'Send {total} book(s) to Kobbler'
            icon_name = None
        elif synced_count == total:
            label = f'Re-send {total} book(s) to Kobbler (all synced)'
            icon_name = 'ok.png'
        else:
            label = f'Send {total} book(s) to Kobbler ({synced_count} already synced)'
            icon_name = 'dialog_warning.png'

        send_action = self.menu.addAction(label)
        send_action.triggered.connect(self.send_selected)
        if icon_name:
            from calibre.gui2 import get_current_app

            icon = get_current_app().get_themed_icon(icon_name)
            if icon:
                send_action.setIcon(icon)

        self.menu.addSeparator()

        # Configure action
        config_action = self.menu.addAction('Configure...')
        config_action.triggered.connect(self.show_config)

    def show_config(self):
        self.interface_action_base_plugin.do_user_config(self.gui)

    def config_widget(self):
        return ConfigWidget()

    def save_settings(self, config_widget):
        config_widget.save_settings()

    def apply_settings(self):
        return

    def send_selected(self):
        if not PREFS['base_url'] or not PREFS['ingest_token']:
            error_dialog(
                self.gui, 'Send to Kobbler', 'Configure base URL and token first.', show=True
            )
            return

        book_ids = list(self.gui.library_view.get_selected_ids())
        if not book_ids:
            info_dialog(self.gui, 'Send to Kobbler', 'No books selected.', show=True)
            return

        db = self.gui.current_db.new_api

        # Separate books into those with KEPUB and those needing conversion
        ready_books = []
        need_conversion = []

        for book_id in book_ids:
            kepub_path = db.format_abspath(book_id, 'KEPUB')
            if kepub_path:
                ready_books.append(book_id)
            else:
                source_fmt = self.pick_source_format(db, book_id)
                if source_fmt:
                    need_conversion.append(book_id)
                else:
                    mi = db.get_metadata(book_id)
                    error_dialog(
                        self.gui,
                        'Send to Kobbler',
                        f'No convertible format for "{mi.title}"',
                        show=True,
                    )
                    return

        if need_conversion:
            # Queue conversions, then upload when done
            self.queue_conversions_then_upload(ready_books, need_conversion)
        else:
            # All books have KEPUB, upload directly
            self.start_upload(ready_books)

    def queue_conversions_then_upload(self, ready_books, need_conversion):
        """Queue conversion jobs for books that need it, then upload all."""
        from calibre.gui2.tools import convert_single_ebook

        batch_id = str(uuid.uuid4())

        # Get conversion jobs using Calibre's infrastructure
        jobs, changed, bad = convert_single_ebook(
            self.gui,
            self.gui.current_db,
            need_conversion,
            auto_conversion=True,
            out_format='KEPUB',
            show_no_format_warning=True,
        )

        if bad:
            db = self.gui.current_db.new_api
            bad_titles = [db.field_for('title', bid) for bid in bad]
            error_dialog(
                self.gui,
                'Send to Kobbler',
                f'Cannot convert: {", ".join(bad_titles)}',
                show=True,
            )
            return

        if not jobs:
            # No conversions needed (maybe all had KEPUB already)
            self.start_upload(ready_books + list(changed))
            return

        # Store batch state
        self._pending_batches[batch_id] = {
            'ready_books': ready_books,
            'converted_book_ids': [],
            'pending_jobs': len(jobs),
            'failed_book_ids': [],
        }

        # Use the Convert Books action's queue_convert_jobs method
        # Pass batch_id via extra_job_args so we can retrieve it in callback
        convert_action = self.gui.iactions['Convert Books']
        convert_action.queue_convert_jobs(
            jobs,
            changed,
            bad,
            need_conversion,  # rows (book_ids)
            None,  # previous
            self._conversion_done,  # converted_func
            extra_job_args=[batch_id],  # passed to callback via conversion_jobs
            rows_are_ids=True,
        )

        total = len(jobs)
        self.gui.status_bar.show_message(f'Kobbler: Converting {total} book(s) to KEPUB...')

    def _conversion_done(self, job):
        """Called when a single conversion job completes."""
        convert_action = self.gui.iactions['Convert Books']

        # Get job data from convert action's tracking
        # Format: [temp_files, fmt, book_id, batch_id]
        if job not in convert_action.conversion_jobs:
            return

        temp_files, fmt, book_id, batch_id = convert_action.conversion_jobs[job][:4]

        if batch_id not in self._pending_batches:
            # Clean up orphaned job
            convert_action.conversion_jobs.pop(job, None)
            return

        batch = self._pending_batches[batch_id]

        # Let Calibre's convert action handle adding the format to library
        convert_action.book_converted(job)

        # Check if conversion succeeded
        db = self.gui.current_db.new_api
        if job.failed:
            batch['failed_book_ids'].append(book_id)
        elif db.format_abspath(book_id, 'KEPUB'):
            batch['converted_book_ids'].append(book_id)
        else:
            batch['failed_book_ids'].append(book_id)

        batch['pending_jobs'] -= 1

        # Check if all conversions done
        if batch['pending_jobs'] <= 0:
            del self._pending_batches[batch_id]

            if batch['failed_book_ids']:
                failed_titles = [db.field_for('title', bid) for bid in batch['failed_book_ids']]
                error_dialog(
                    self.gui,
                    'Send to Kobbler',
                    f'Conversion failed for: {", ".join(failed_titles)}',
                    show=True,
                )

            # Start upload with all books that have KEPUB
            all_books = batch['ready_books'] + batch['converted_book_ids']
            if all_books:
                self.start_upload(all_books)

    def start_upload(self, book_ids):
        """Start the upload job for books that are ready."""
        db = self.gui.current_db.new_api
        book_data_list = []

        for book_id in book_ids:
            try:
                book_data = self.prepare_book_data(db, book_id)
                book_data_list.append(book_data)
            except Exception as exc:
                error_dialog(
                    self.gui,
                    'Send to Kobbler',
                    f'Failed to prepare book {book_id}: {exc}',
                    show=True,
                )
                return

        base_url = PREFS['base_url']
        ingest_token = PREFS['ingest_token']

        func = partial(run_upload_job, book_data_list, base_url, ingest_token)
        job = ThreadedJob(
            'kobbler_upload',
            f'Kobbler: Uploading {len(book_ids)} book(s)',
            func,
            {},
            {},
            self.upload_done,
        )
        self.gui.job_manager.run_threaded_job(job)
        self.gui.status_bar.show_message(f'Kobbler: Uploading {len(book_ids)} book(s)...')

    def prepare_book_data(self, db, book_id):
        """Prepare book data for upload."""
        mi = db.get_metadata(book_id, get_cover=True, cover_as_data=True)

        identifiers = dict(mi.identifiers or {})
        if 'kobbler' in identifiers:
            book_uuid = identifiers['kobbler']
        elif mi.uuid:
            book_uuid = mi.uuid
        else:
            book_uuid = str(uuid.uuid4())
            identifiers['kobbler'] = book_uuid
            db.set_field('identifiers', {book_id: identifiers})

        kepub_path = db.format_abspath(book_id, 'KEPUB')
        if not kepub_path:
            raise Exception('KEPUB format not available')

        cover_bytes = None
        if mi.cover_data and len(mi.cover_data) >= 2:
            cover_bytes = mi.cover_data[1]

        # Extract external identifiers (exclude internal 'kobbler' ID)
        external_identifiers = {k: v for k, v in identifiers.items() if k != 'kobbler'}

        return {
            'calibre_id': book_id,
            'book_uuid': book_uuid,
            'title': mi.title,
            'description': mi.comments,
            'language': mi.language,
            'publisher': mi.publisher,
            'pubdate': mi.pubdate.isoformat() if mi.pubdate else None,
            'series_name': mi.series,
            'series_index': float(mi.series_index) if mi.series_index else None,
            'authors': list(mi.authors or []),
            'identifiers': external_identifiers,
            'cover_bytes': cover_bytes,
            'kepub_path': kepub_path,
            'cleanup_kepub': False,
        }

    def pick_source_format(self, db, book_id):
        """Find best source format for conversion."""
        formats = db.formats(book_id)
        if not formats:
            return None
        available = [fmt.upper() for fmt in formats]
        preferred = ['EPUB', 'MOBI', 'AZW3', 'PDF', 'DOCX', 'RTF', 'TXT']
        for fmt in preferred:
            if fmt in available:
                return fmt
        for fmt in available:
            if fmt == 'KEPUB':
                continue
            return fmt
        return None

    def upload_done(self, job):
        """Handle upload job completion - called from background thread."""
        # Store result for main thread to process
        self._last_upload_result = {
            'failed': job.failed,
            'result': job.result if not job.failed else None,
            'details': job.details if job.failed else None,
        }
        # Schedule GUI update on main thread using QTimer.singleShot
        if QTimer is not None:
            QTimer.singleShot(0, self._show_upload_result)
        else:
            # Fallback if QTimer not available (shouldn't happen)
            self._show_upload_result()

    def _show_upload_result(self):
        """Show upload result dialog - runs on main thread."""
        result_data = getattr(self, '_last_upload_result', None)
        if result_data is None:
            return

        self._last_upload_result = None

        if result_data['failed']:
            error_dialog(
                self.gui,
                'Send to Kobbler',
                'Upload job failed. Check the job details for more info.',
                det_msg=result_data['details'],
                show=True,
            )
            return

        result = result_data['result'] or {}
        failures = result.get('failures', [])
        synced_book_ids = result.get('synced_book_ids', [])

        if failures:
            msg = '\n'.join([f'{title}: {err}' for bid, title, err in failures])
            error_dialog(
                self.gui,
                'Send to Kobbler',
                f'Upload failed for {len(failures)} book(s):',
                det_msg=msg,
                show=True,
            )
        else:
            total = len(synced_book_ids)
            info_dialog(
                self.gui,
                'Send to Kobbler',
                f'Successfully uploaded {total} book(s) to Kobbler.',
                show=True,
            )
