// ═══════════════════════════════════════════════════════════
//  CINEMATIC — Main Application Logic
// ═══════════════════════════════════════════════════════════

const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const appWindow = getCurrentWindow();

// ─── Custom Titlebar ───
document.getElementById('titlebar-minimize').addEventListener('click', () => appWindow.minimize());
document.getElementById('titlebar-maximize').addEventListener('click', async () => {
    const isMaximized = await appWindow.isMaximized();
    if (isMaximized) {
        await appWindow.unmaximize();
    } else {
        await appWindow.maximize();
    }
});
document.getElementById('titlebar-close').addEventListener('click', () => appWindow.close());

// Double-click titlebar to maximize/restore
document.getElementById('custom-titlebar').addEventListener('dblclick', async (e) => {
    if (e.target.closest('.titlebar-controls')) return;
    const isMaximized = await appWindow.isMaximized();
    if (isMaximized) {
        await appWindow.unmaximize();
    } else {
        await appWindow.maximize();
    }
});
// ─── State ───
let allVideos = [];
let libraries = [];
let collections = [];
let currentView = 'home';
let currentLibraryId = null;
let currentCollectionId = null;
let selectedVideo = null;
let searchQuery = '';
let sortMode = 'date-desc';
let isListView = false;
let thumbnailCache = {};
let collectionCoverCache = {};
let syncVideosPromise = null;
let activeVideoMenuId = null;
let collectionPickerVideo = null;
let collectionThumbnailTarget = null;
let featuredVideoId = null;

// ─── Initialize ───
document.addEventListener('DOMContentLoaded', async () => {
    await loadData();
    setupEventListeners();
    setupScrollReveal();
    renderCurrentView();
});

// ─── Data Loading ───
async function loadData() {
    try {
        [libraries, allVideos, collections] = await Promise.all([
            invoke('get_libraries'),
            invoke('get_all_videos'),
            invoke('get_collections'),
        ]);
        renderLibraryNav();
        renderCollectionNav();
        updateStats();
        // Start loading thumbnails in background
        loadThumbnailsBatch();
        loadCollectionCoversBatch();
        // Extract metadata for videos without it
        extractMetadataBatch();
    } catch (e) {
        console.error('Failed to load data:', e);
        showToast('Failed to load library data', 'error');
    }
}

async function loadThumbnailsBatch() {
    for (const video of allVideos) {
        if (video.thumbnail_path && !thumbnailCache[video.id]) {
            try {
                const base64 = await invoke('get_thumbnail_base64', { thumbnailPath: video.thumbnail_path });
                thumbnailCache[video.id] = base64;
                // Update any visible card thumbnails
                updateCardThumbnail(video.id, base64);
            } catch (e) {
                // Thumbnail not available, skip
            }
        }
    }
}

async function loadCollectionCoversBatch() {
    let loadedAny = false;
    for (const collection of collections) {
        if (collection.cover_image_path && !collectionCoverCache[collection.id]) {
            try {
                const base64 = await invoke('get_image_base64', { imagePath: collection.cover_image_path });
                collectionCoverCache[collection.id] = base64;
                loadedAny = true;
            } catch (e) {
                // Cover image not available, skip
            }
        }
    }

    if (loadedAny && currentView === 'home') {
        renderHome();
    }
}

async function extractMetadataBatch() {
    const needsMeta = allVideos.filter(v => !v.duration_secs);
    for (const video of needsMeta.slice(0, 30)) {
        try {
            await invoke('extract_video_metadata', { videoId: video.id, videoPath: video.path });
        } catch (e) {
            // ignore
        }
    }
    // Reload videos after metadata extraction
    if (needsMeta.length > 0) {
        allVideos = await invoke('get_all_videos');
        renderCurrentView();
        // Generate thumbnails for newly extracted
        try {
            const count = await invoke('generate_thumbnails');
            if (count > 0) {
                allVideos = await invoke('get_all_videos');
                renderCurrentView();
                loadThumbnailsBatch();
            }
        } catch (e) {
            // ignore
        }
    }
}

async function syncVideoState() {
    if (syncVideosPromise) return syncVideosPromise;

    syncVideosPromise = (async () => {
        try {
            allVideos = await invoke('get_all_videos');
            collections = await invoke('get_collections');
            renderCollectionNav();
            const nextCoverCache = {};
            collections.forEach(collection => {
                if (collection.cover_image_path && collectionCoverCache[collection.id]) {
                    nextCoverCache[collection.id] = collectionCoverCache[collection.id];
                }
            });
            collectionCoverCache = nextCoverCache;
            loadCollectionCoversBatch();
            updateStats();

            if (selectedVideo) {
                const latest = allVideos.find(v => v.id === selectedVideo.id);
                if (latest) {
                    selectedVideo = latest;
                    updateWatchedButton(selectedVideo);
                    document.getElementById('modal-favorite').classList.toggle('active', selectedVideo.favorite === 1);
                    updatePlayButton(selectedVideo);
                }
            }

            renderCurrentView();
        } catch (e) {
            console.error('Failed to sync video state:', e);
        } finally {
            syncVideosPromise = null;
        }
    })();

    return syncVideosPromise;
}

function updateCardThumbnail(videoId, base64) {
    const imgs = document.querySelectorAll(`[data-video-id="${videoId}"] .card-thumbnail img`);
    imgs.forEach(img => {
        img.src = base64;
        img.style.display = 'block';
        const placeholder = img.parentElement.querySelector('.card-thumbnail-placeholder');
        if (placeholder) placeholder.style.display = 'none';
    });
    // Also update modal if open
    const modalImg = document.querySelector('#modal-thumbnail img');
    if (modalImg && selectedVideo && selectedVideo.id === videoId) {
        modalImg.src = base64;
    }
}

function closeVideoMenus() {
    document.querySelectorAll('.video-card.menu-open').forEach(card => card.classList.remove('menu-open'));
    document.querySelectorAll('.collection-home-card.menu-open').forEach(card => card.classList.remove('menu-open'));
    document.querySelectorAll('.card-menu-trigger.active').forEach(btn => btn.classList.remove('active'));
    document.querySelectorAll('.card-overflow-menu.open').forEach(menu => menu.classList.remove('open'));
    activeVideoMenuId = null;
}

function toggleOverflowMenu(trigger) {
    const card = trigger.closest('.video-card, .collection-home-card');
    const menu = card ? card.querySelector('.card-overflow-menu') : null;
    if (!menu) return;

    const menuId = trigger.dataset.videoMenuTrigger || trigger.dataset.collectionMenuTrigger;
    const isOpen = activeVideoMenuId === menuId && menu.classList.contains('open');
    closeVideoMenus();

    if (!isOpen) {
        if (card) card.classList.add('menu-open');
        trigger.classList.add('active');
        menu.classList.add('open');
        activeVideoMenuId = menuId;
    }
}

async function deleteVideoPermanently(video) {
    const confirmed = window.confirm(`Delete "${video.title}" permanently from disk?\n\nThis cannot be undone.`);
    if (!confirmed) return;

    try {
        await invoke('delete_video_file', { videoId: video.id });
        allVideos = allVideos.filter(v => v.id !== video.id);
        delete thumbnailCache[video.id];
        collections = await invoke('get_collections');
        renderCollectionNav();
        updateStats();
        closeVideoMenus();

        if (selectedVideo && selectedVideo.id === video.id) {
            closeModal();
        }

        renderCurrentView();
        showToast('Video deleted permanently', 'success');
    } catch (e) {
        showToast('Failed to delete video: ' + e, 'error');
    }
}

function handleVideoMenuAction(action, video) {
    switch (action) {
        case 'collections':
            openCollectionPickerDialog(video);
            break;
        case 'details':
            openModal(video);
            break;
        case 'delete':
            deleteVideoPermanently(video);
            break;
    }
}

async function handleCollectionMenuAction(action, collectionId) {
    const collection = collections.find(c => c.id === collectionId);
    if (!collection) return;

    switch (action) {
        case 'open':
            switchView('collection', { collectionId });
            break;
        case 'thumbnail':
            await openCollectionThumbnailDialog(collection);
            break;
    }
}

// ─── Event Listeners ───
function setupEventListeners() {
    // Sidebar collapse/expand
    const sidebar = document.getElementById('sidebar');
    const collapseBtn = document.getElementById('sidebar-collapse-btn');
    const expandBtn = document.getElementById('sidebar-expand-btn');

    // Restore persisted state
    if (localStorage.getItem('sidebar-collapsed') === 'true') {
        sidebar.classList.add('collapsed');
    }

    function toggleSidebar() {
        const isCollapsed = sidebar.classList.contains('collapsed');
        if (sidebar.dataset.hoverExpanded === 'true') {
            if (!isCollapsed) {
               sidebar.classList.remove('is-hovering');
               sidebar.classList.add('collapsed');
               localStorage.setItem('sidebar-collapsed', 'true');
               sidebar.dataset.hoverExpanded = 'false';
               return;
            }
        }
        sidebar.classList.remove('is-hovering');
        sidebar.classList.toggle('collapsed');
        localStorage.setItem('sidebar-collapsed', sidebar.classList.contains('collapsed'));
        sidebar.dataset.hoverExpanded = 'false';
    }

    collapseBtn.addEventListener('click', toggleSidebar);
    expandBtn.addEventListener('click', toggleSidebar);

    // Sidebar auto-expand settings
    const autoExpandToggle = document.getElementById('setting-auto-expand-sidebar');
    if (autoExpandToggle) {
        autoExpandToggle.checked = localStorage.getItem('sidebar-auto-expand') === 'true';
        autoExpandToggle.addEventListener('change', (e) => {
            localStorage.setItem('sidebar-auto-expand', e.target.checked);
        });
    }

    // Sidebar auto-expand hover logic
    let hoverExpandTimeout = null;

    sidebar.addEventListener('mouseenter', () => {
        if (localStorage.getItem('sidebar-auto-expand') === 'true' && sidebar.classList.contains('collapsed')) {
            clearTimeout(hoverExpandTimeout);
            hoverExpandTimeout = setTimeout(() => {
                sidebar.dataset.hoverExpanded = 'true';
                sidebar.classList.add('is-hovering');
                sidebar.classList.remove('collapsed');
            }, 180); // Slight intentional delay before popping open
        }
    });

    sidebar.addEventListener('mouseleave', () => {
        clearTimeout(hoverExpandTimeout); // Cancel hover expansion if mouse leaves before timeout
        if (sidebar.dataset.hoverExpanded === 'true') {
            sidebar.dataset.hoverExpanded = 'false';
            sidebar.classList.remove('is-hovering');
            if (localStorage.getItem('sidebar-collapsed') === 'true') {
                sidebar.classList.add('collapsed');
            }
        }
    });

    // Add title attributes to nav items for collapsed tooltip
    document.querySelectorAll('.nav-item[data-view]').forEach(btn => {
        const label = btn.querySelector('span');
        if (label) btn.title = label.textContent;
    });

    // Navigation
    document.querySelectorAll('.nav-item[data-view]').forEach(btn => {
        btn.addEventListener('click', () => {
            const view = btn.dataset.view;
            if (view) switchView(view);
        });
    });

    // Add Library buttons
    document.getElementById('btn-add-library').addEventListener('click', addLibrary);
    document.getElementById('hero-add-library').addEventListener('click', addLibrary);
    document.getElementById('empty-add-library').addEventListener('click', addLibrary);
    document.getElementById('settings-add-library').addEventListener('click', addLibrary);

    // Scan all
    document.getElementById('hero-scan-all').addEventListener('click', scanAllLibraries);

    // Refresh
    document.getElementById('btn-refresh').addEventListener('click', refreshLibrary);

    // Search
    document.getElementById('search-input').addEventListener('input', (e) => {
        searchQuery = e.target.value.toLowerCase();
        renderCurrentView();
    });

    // Sort
    document.getElementById('sort-select').addEventListener('change', (e) => {
        sortMode = e.target.value;
        renderCurrentView();
    });

    // View Toggle
    document.getElementById('btn-grid-view').addEventListener('click', () => {
        isListView = false;
        document.getElementById('btn-grid-view').classList.add('active');
        document.getElementById('btn-list-view').classList.remove('active');
        renderCurrentView();
    });
    document.getElementById('btn-list-view').addEventListener('click', () => {
        isListView = true;
        document.getElementById('btn-list-view').classList.add('active');
        document.getElementById('btn-grid-view').classList.remove('active');
        renderCurrentView();
    });

    // Modal
    document.getElementById('modal-backdrop').addEventListener('click', closeModal);
    document.getElementById('modal-close').addEventListener('click', closeModal);
    document.getElementById('modal-play').addEventListener('click', () => {
        if (selectedVideo) playVideo(selectedVideo);
    });
    document.getElementById('modal-watched').addEventListener('click', toggleWatchedModal);
    document.getElementById('modal-favorite').addEventListener('click', toggleFavoriteModal);

    // Collections dialog
    document.getElementById('btn-create-collection').addEventListener('click', showNewCollectionDialog);
    document.getElementById('dialog-cancel-collection').addEventListener('click', () => {
        document.getElementById('dialog-new-collection').style.display = 'none';
    });
    document.getElementById('dialog-create-collection').addEventListener('click', createCollection);
    document.getElementById('collection-picker-close').addEventListener('click', closeCollectionPickerDialog);
    document.getElementById('dialog-collection-picker-backdrop').addEventListener('click', closeCollectionPickerDialog);
    document.getElementById('collection-picker-create-btn').addEventListener('click', createCollectionFromPicker);
    document.getElementById('collection-thumbnail-close').addEventListener('click', closeCollectionThumbnailDialog);
    document.getElementById('dialog-collection-thumbnail-backdrop').addEventListener('click', closeCollectionThumbnailDialog);
    document.getElementById('collection-thumbnail-clear').addEventListener('click', clearCollectionThumbnail);
    document.getElementById('collection-thumbnail-upload').addEventListener('click', uploadCollectionThumbnailImage);
    document.getElementById('collection-picker-name').addEventListener('keydown', (e) => {
        if (e.key === 'Enter') {
            e.preventDefault();
            createCollectionFromPicker();
        }
    });

    window.addEventListener('focus', () => {
        syncVideoState();
    });

    document.addEventListener('click', (e) => {
        if (!e.target.closest('.card-menu-trigger') && !e.target.closest('.card-overflow-menu')) {
            closeVideoMenus();
        }
    });

    // Keyboard
    document.addEventListener('keydown', (e) => {
        if (e.key === 'Escape') {
            closeVideoMenus();
            closeModal();
            document.getElementById('dialog-new-collection').style.display = 'none';
            closeCollectionPickerDialog();
            closeCollectionThumbnailDialog();
        }
        // Ctrl+B to toggle sidebar
        if ((e.ctrlKey || e.metaKey) && e.key === 'b') {
            e.preventDefault();
            toggleSidebar();
        }
    });
}

// ─── Navigation ───
function switchView(view, data = {}) {
    currentView = view;
    currentLibraryId = data.libraryId || null;
    currentCollectionId = data.collectionId || null;

    // Update nav active state
    document.querySelectorAll('.nav-item[data-view]').forEach(btn => {
        btn.classList.toggle('active', btn.dataset.view === view);
    });
    document.querySelectorAll('.nav-item-library').forEach(btn => {
        btn.classList.toggle('active', view === 'library' && btn.dataset.libraryId === currentLibraryId);
    });
    document.querySelectorAll('.nav-item-collection').forEach(btn => {
        btn.classList.toggle('active', view === 'collection' && btn.dataset.collectionId === currentCollectionId);
    });

    // Show panel
    document.querySelectorAll('.view-panel').forEach(panel => {
        panel.classList.remove('active');
    });

    const panel = document.getElementById(`view-${view}`);
    if (panel) {
        panel.classList.add('active');
    }

    renderCurrentView();
    
    // Scroll to top
    document.getElementById('content-area').scrollTop = 0;
}

function renderCurrentView() {
    switch (currentView) {
        case 'home': renderHome(); break;
        case 'all': renderAllVideos(); break;
        case 'recent': renderRecent(); break;
        case 'unwatched': renderUnwatched(); break;
        case 'favorites': renderFavorites(); break;
        case 'library': renderLibraryView(); break;
        case 'collection': renderCollectionView(); break;
        case 'settings': renderSettings(); break;
    }
}

// ─── Render: Home ───
function renderHome() {
    // Update hero
    const heroSubtitle = document.getElementById('hero-subtitle');
    if (allVideos.length > 0) {
        const watchedCount = allVideos.filter(v => v.watched).length;
        heroSubtitle.textContent = `${allVideos.length} videos across ${libraries.length} ${libraries.length === 1 ? 'library' : 'libraries'} · ${watchedCount} watched`;
    } else {
        heroSubtitle.textContent = 'Add a library folder to start browsing your collection';
    }

    // Featured card
    const heroVisual = document.getElementById('hero-visual');
    const featured = getFeaturedVideo();
    if (featured) {
        heroVisual.innerHTML = createFeaturedCard(featured);
    } else {
        heroVisual.innerHTML = '';
    }

    // Continue Watching shelf
    const continueWatching = allVideos.filter(v => v.watch_progress_secs > 0 && !v.watched);
    const shelfContinue = document.getElementById('shelf-continue');
    if (continueWatching.length > 0) {
        shelfContinue.style.display = 'block';
        document.getElementById('shelf-continue-items').innerHTML = continueWatching.slice(0, 12).map(createVideoCard).join('');
    } else {
        shelfContinue.style.display = 'none';
    }

    // Collections shelf
    const shelfCollections = document.getElementById('shelf-collections');
    if (collections.length > 0) {
        shelfCollections.style.display = 'block';
        document.getElementById('shelf-collection-items').innerHTML = collections.map(createCollectionCard).join('');
    } else {
        shelfCollections.style.display = 'none';
    }

    // Recently Added shelf
    const recentItems = [...allVideos].sort((a, b) => b.date_added.localeCompare(a.date_added)).slice(0, 12);
    document.getElementById('shelf-recent-items').innerHTML = recentItems.map(createVideoCard).join('');

    // Unwatched shelf
    const unwatched = allVideos.filter(v => !v.watched).slice(0, 12);
    document.getElementById('shelf-unwatched-items').innerHTML = unwatched.map(createVideoCard).join('');

    attachCardListeners();
    attachCollectionCardListeners();
    applyScrollReveal();
}

function getFeaturedVideo() {
    if (allVideos.length === 0) {
        featuredVideoId = null;
        return null;
    }

    if (featuredVideoId) {
        const existingFeatured = allVideos.find(video => video.id === featuredVideoId);
        if (existingFeatured) {
            return existingFeatured;
        }
    }

    const randomVideo = allVideos[Math.floor(Math.random() * allVideos.length)];
    featuredVideoId = randomVideo.id;
    return randomVideo;
}

function renderAllVideos() {
    const filtered = filterAndSort(allVideos);
    document.getElementById('all-count').textContent = `${filtered.length} videos`;
    const grid = document.getElementById('grid-all');
    grid.className = `video-grid${isListView ? ' list-view' : ''}`;
    grid.innerHTML = filtered.map(createVideoCard).join('');
    attachCardListeners();
    applyScrollReveal();
}

function renderRecent() {
    const recent = filterAndSort([...allVideos].sort((a, b) => b.date_added.localeCompare(a.date_added)).slice(0, 50));
    const grid = document.getElementById('grid-recent');
    grid.className = `video-grid${isListView ? ' list-view' : ''}`;
    grid.innerHTML = recent.map(createVideoCard).join('');
    attachCardListeners();
    applyScrollReveal();
}

function renderUnwatched() {
    const unwatched = filterAndSort(allVideos.filter(v => !v.watched));
    document.getElementById('unwatched-count').textContent = `${unwatched.length} unwatched videos`;
    const grid = document.getElementById('grid-unwatched');
    grid.className = `video-grid${isListView ? ' list-view' : ''}`;
    grid.innerHTML = unwatched.map(createVideoCard).join('');
    attachCardListeners();
    applyScrollReveal();
}

function renderFavorites() {
    const favorites = filterAndSort(allVideos.filter(v => v.favorite));
    const grid = document.getElementById('grid-favorites');
    grid.className = `video-grid${isListView ? ' list-view' : ''}`;
    grid.innerHTML = favorites.length > 0 ? favorites.map(createVideoCard).join('') :
        '<div class="empty-state"><div class="empty-icon"><svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="0.75" opacity="0.3"><path d="M20.84 4.61a5.5 5.5 0 00-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 00-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 000-7.78z"/></svg></div><h2 class="empty-title">No Favorites Yet</h2><p class="empty-text">Mark videos as favorites to see them here</p></div>';
    attachCardListeners();
    applyScrollReveal();
}

async function renderLibraryView() {
    if (!currentLibraryId) return;
    const lib = libraries.find(l => l.id === currentLibraryId);
    if (lib) {
        document.getElementById('library-view-title').textContent = lib.name;
        document.getElementById('library-view-path').textContent = lib.path;
    }
    const libVideos = filterAndSort(allVideos.filter(v => v.library_id === currentLibraryId));
    const grid = document.getElementById('grid-library');
    grid.className = `video-grid${isListView ? ' list-view' : ''}`;
    grid.innerHTML = libVideos.map(createVideoCard).join('');
    attachCardListeners();
    applyScrollReveal();
}

async function renderCollectionView() {
    if (!currentCollectionId) return;
    const col = collections.find(c => c.id === currentCollectionId);
    if (col) {
        document.getElementById('collection-view-title').textContent = col.name;
        document.getElementById('collection-view-desc').textContent = col.description || `${col.video_count} videos`;
    }
    try {
        const colVideos = await invoke('get_collection_videos', { collectionId: currentCollectionId });
        const filtered = filterAndSort(colVideos);
        const grid = document.getElementById('grid-collection');
        grid.className = `video-grid${isListView ? ' list-view' : ''}`;
        grid.innerHTML = filtered.length > 0 ? filtered.map(createVideoCard).join('') :
            '<div class="empty-state"><h2 class="empty-title">Empty Collection</h2><p class="empty-text">Add videos to this collection from the video detail view</p></div>';
        attachCardListeners();
        applyScrollReveal();
    } catch (e) {
        console.error(e);
    }
}

function renderSettings() {
    const list = document.getElementById('settings-libraries-list');
    list.innerHTML = libraries.map(lib => `
        <div class="settings-library-item">
            <div class="settings-library-info">
                <h4>${escapeHtml(lib.name)}</h4>
                <p>${escapeHtml(lib.path)}</p>
            </div>
            <div class="settings-library-actions">
                <button class="btn-scan" data-library-id="${lib.id}" data-library-path="${escapeHtml(lib.path)}">Scan</button>
                <button class="btn-remove" data-library-id="${lib.id}">Remove</button>
            </div>
        </div>
    `).join('');

    // Attach listeners
    list.querySelectorAll('.btn-scan').forEach(btn => {
        btn.addEventListener('click', async () => {
            await scanLibrary(btn.dataset.libraryId, btn.dataset.libraryPath);
        });
    });
    list.querySelectorAll('.btn-remove').forEach(btn => {
        btn.addEventListener('click', async () => {
            await removeLibrary(btn.dataset.libraryId);
        });
    });
}

// ─── Card Creation ───
function createVideoCard(video) {
    const thumbnail = thumbnailCache[video.id];
    const duration = formatDuration(video.duration_secs);
    const fileSize = formatFileSize(video.file_size);
    const resolution = video.width && video.height ? `${video.width}×${video.height}` : '';
    const isWatched = video.watched === 1;
    const isFavorite = video.favorite === 1;
    const progress = video.duration_secs && video.watch_progress_secs > 0 
        ? (video.watch_progress_secs / video.duration_secs) * 100 
        : 0;

    return `
        <div class="video-card reveal-item" data-video-id="${video.id}" title="${escapeHtml(video.title)}">
            <button class="card-menu-trigger" data-video-menu-trigger="${video.id}" aria-label="More actions">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="12" cy="19" r="2"/></svg>
            </button>
            <div class="card-overflow-menu" data-video-menu="${video.id}">
                <button type="button" data-video-action="collections">Add To Collection</button>
                <button type="button" data-video-action="details">Details</button>
                <button type="button" class="danger" data-video-action="delete">Delete Permanently</button>
            </div>
            <div class="card-thumbnail">
                ${thumbnail ? 
                    `<img src="${thumbnail}" alt="${escapeHtml(video.title)}" loading="lazy" />` :
                    `<img src="" alt="" style="display:none" /><div class="card-thumbnail-placeholder"><svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="0.75"><rect x="2" y="4" width="20" height="16" rx="3"/><polygon points="10,8 16,12 10,16"/></svg></div>`
                }
                <div class="card-play">
                    <svg width="16" height="16" viewBox="0 0 24 24"><polygon points="8,4 20,12 8,20"/></svg>
                </div>
                ${isWatched ? '<div class="card-watched-badge">Watched</div>' : ''}
                ${isFavorite ? '<div class="card-favorite"><svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" stroke="none"><path d="M20.84 4.61a5.5 5.5 0 00-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 00-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 000-7.78z"/></svg></div>' : ''}
                ${duration ? `<div class="card-duration">${duration}</div>` : ''}
                ${progress > 0 && !isWatched ? `<div class="card-progress"><div class="card-progress-bar" style="width: ${progress}%"></div></div>` : ''}
            </div>
            <div class="card-info">
                <div class="card-title">${escapeHtml(video.title)}</div>
                <div class="card-meta">
                    ${fileSize ? `<span>${fileSize}</span>` : ''}
                    ${fileSize && resolution ? '<span class="card-meta-dot"></span>' : ''}
                    ${resolution ? `<span>${resolution}</span>` : ''}
                </div>
            </div>
        </div>
    `;
}

function createFeaturedCard(video) {
    const thumbnail = thumbnailCache[video.id];
    const duration = formatDuration(video.duration_secs);
    const fileSize = formatFileSize(video.file_size);

    return `
        <div class="featured-card" data-video-id="${video.id}">
            ${thumbnail ? 
                `<img src="${thumbnail}" alt="${escapeHtml(video.title)}" />` :
                `<div class="card-thumbnail-placeholder" style="width:100%;height:100%"><svg width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="0.5"><rect x="2" y="4" width="20" height="16" rx="3"/><polygon points="10,8 16,12 10,16"/></svg></div>`
            }
            <div class="featured-card-overlay">
                <div class="featured-card-title">${escapeHtml(video.title)}</div>
                <div class="featured-card-meta">${[duration, fileSize].filter(Boolean).join(' · ')}</div>
            </div>
            <div class="featured-play-icon">
                <svg width="20" height="20" viewBox="0 0 24 24" fill="white"><polygon points="8,4 20,12 8,20"/></svg>
            </div>
        </div>
    `;
}

function createCollectionCard(collection) {
    const description = collection.description?.trim() || '';
    const coverThumbnail = collection.cover_image_path
        ? collectionCoverCache[collection.id]
        : (collection.cover_video_id ? thumbnailCache[collection.cover_video_id] : null);
    return `
        <div class="collection-home-card reveal-item" data-collection-id="${collection.id}" title="${escapeHtml(collection.name)}">
            ${coverThumbnail ? `
                <div class="collection-home-cover">
                    <img src="${coverThumbnail}" alt="${escapeHtml(collection.name)}" />
                </div>
                <div class="collection-home-cover-overlay"></div>
            ` : ''}
            <button class="card-menu-trigger collection-card-menu-trigger" data-collection-menu-trigger="${collection.id}" aria-label="Collection actions">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="12" cy="19" r="2"/></svg>
            </button>
            <div class="card-overflow-menu" data-collection-menu="${collection.id}">
                <button type="button" data-collection-action="open">Open Collection</button>
                <button type="button" data-collection-action="thumbnail">Set Thumbnail</button>
            </div>
            ${coverThumbnail ? `
                <div class="collection-home-icon-spacer" aria-hidden="true"></div>
            ` : `
                <div class="collection-home-icon">
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="2" y="7" width="20" height="14" rx="2"/><path d="M16 3h-8l-2 4h12l-2-4z"/></svg>
                </div>
            `}
            <div class="collection-home-info">
                <div class="collection-home-title">${escapeHtml(collection.name)}</div>
                <div class="collection-home-meta">${collection.video_count} ${collection.video_count === 1 ? 'video' : 'videos'}</div>
                ${description ? `<div class="collection-home-desc">${escapeHtml(description)}</div>` : ''}
            </div>
        </div>
    `;
}

function attachCollectionCardListeners() {
    document.querySelectorAll('[data-collection-menu-trigger]').forEach(trigger => {
        trigger.addEventListener('click', (e) => {
            e.stopPropagation();
            toggleOverflowMenu(trigger);
        });
    });

    document.querySelectorAll('[data-collection-menu]').forEach(menu => {
        menu.addEventListener('click', async (e) => {
            e.stopPropagation();
            const actionButton = e.target.closest('[data-collection-action]');
            if (!actionButton) return;
            closeVideoMenus();
            await handleCollectionMenuAction(actionButton.dataset.collectionAction, menu.dataset.collectionMenu);
        });
    });

    document.querySelectorAll('.collection-home-card').forEach(card => {
        card.addEventListener('click', (e) => {
            if (e.target.closest('.card-menu-trigger') || e.target.closest('.card-overflow-menu')) {
                return;
            }
            switchView('collection', { collectionId: card.dataset.collectionId });
        });
    });
}

function attachCardListeners() {
    document.querySelectorAll('[data-video-menu-trigger]').forEach(trigger => {
        trigger.addEventListener('click', (e) => {
            e.stopPropagation();
            toggleOverflowMenu(trigger);
        });
    });

    document.querySelectorAll('[data-video-menu]').forEach(menu => {
        menu.addEventListener('click', (e) => {
            e.stopPropagation();
            const actionButton = e.target.closest('[data-video-action]');
            if (!actionButton) return;

            const videoId = menu.dataset.videoMenu;
            const video = allVideos.find(v => v.id === videoId);
            if (!video) return;

            closeVideoMenus();
            handleVideoMenuAction(actionButton.dataset.videoAction, video);
        });
    });

    document.querySelectorAll('.video-card').forEach(card => {
        card.addEventListener('click', (e) => {
            const videoId = card.dataset.videoId;
            const video = allVideos.find(v => v.id === videoId);
            if (!video) return;

            if (e.target.closest('.card-menu-trigger') || e.target.closest('.card-overflow-menu')) {
                return;
            }

            // Open modal only if clicking on the title
            if (e.target.closest('.card-title')) {
                openModal(video);
                return;
            }

            // Reuse the modal playback path so card clicks and the explicit play action stay in sync.
            playVideo(video);
        });
    });
    
    document.querySelectorAll('.featured-card').forEach(card => {
        card.addEventListener('click', (e) => {
            const videoId = card.dataset.videoId;
            const video = allVideos.find(v => v.id === videoId);
            if (!video) return;

            if (e.target.closest('.card-menu-trigger') || e.target.closest('.card-overflow-menu')) {
                return;
            }

            if (e.target.closest('.featured-card-title')) {
                openModal(video);
                return;
            }

            // Reuse the modal playback path so card clicks and the explicit play action stay in sync.
            playVideo(video);
        });
    });
}

// ─── Modal ───
function getResumeSeconds(video) {
    if (!video || video.watched === 1) return 0;
    return Math.max(0, video.watch_progress_secs || 0);
}

function updatePlayButton(video) {
    const button = document.getElementById('modal-play');
    const label = button.querySelector('span');
    if (!label) return;

    const resumeSecs = getResumeSeconds(video);
    label.textContent = resumeSecs >= 5
        ? `Resume in VLC (${formatDuration(resumeSecs)})`
        : 'Play in VLC';
}

function openModal(video) {
    closeVideoMenus();
    selectedVideo = video;
    const overlay = document.getElementById('modal-overlay');
    overlay.style.display = 'flex';

    // Thumbnail
    const thumbEl = document.getElementById('modal-thumbnail');
    const thumbnail = thumbnailCache[video.id];
    thumbEl.innerHTML = thumbnail ?
        `<img src="${thumbnail}" alt="${escapeHtml(video.title)}" />` :
        `<div class="modal-thumbnail-placeholder"><svg width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="0.5" opacity="0.2"><rect x="2" y="4" width="20" height="16" rx="3"/><polygon points="10,8 16,12 10,16"/></svg></div>`;

    // Title
    document.getElementById('modal-title').textContent = video.title;

    // Meta
    const meta = [];
    if (video.duration_secs) meta.push(`<span class="modal-meta-item"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 3"/></svg>${formatDuration(video.duration_secs)}</span>`);
    if (video.file_size) meta.push(`<span class="modal-meta-item"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M13 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V9z"/><path d="M13 2v7h7"/></svg>${formatFileSize(video.file_size)}</span>`);
    if (video.width && video.height) meta.push(`<span class="modal-meta-item"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8m-4-4v4"/></svg>${video.width}×${video.height}</span>`);
    const lib = libraries.find(l => l.id === video.library_id);
    if (lib) meta.push(`<span class="modal-meta-item"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>${escapeHtml(lib.name)}</span>`);
    document.getElementById('modal-meta').innerHTML = meta.join('');

    // Path
    document.getElementById('modal-path').textContent = video.path;

    // Watched button
    const watchedBtn = document.getElementById('modal-watched');
    updateWatchedButton(video);

    // Favorite button
    const favBtn = document.getElementById('modal-favorite');
    favBtn.classList.toggle('active', video.favorite === 1);

    updatePlayButton(video);

    // Collections
    renderModalCollections(video);
}

function closeModal() {
    document.getElementById('modal-overlay').style.display = 'none';
    selectedVideo = null;
}

function updateWatchedButton(video) {
    const btn = document.getElementById('modal-watched');
    btn.innerHTML = `<span>${video.watched ? 'Mark Unwatched' : 'Mark Watched'}</span>`;
}

async function refreshCollectionsState() {
    collections = await invoke('get_collections');
    renderCollectionNav();
    if (currentView === 'collection') {
        await renderCollectionView();
    } else if (currentView === 'home') {
        renderHome();
    }
}

function closeCollectionPickerDialog() {
    document.getElementById('dialog-collection-picker').style.display = 'none';
    document.getElementById('collection-picker-name').value = '';
    collectionPickerVideo = null;
}

function closeCollectionThumbnailDialog() {
    document.getElementById('dialog-collection-thumbnail').style.display = 'none';
    document.getElementById('collection-thumbnail-grid').innerHTML = '';
    collectionThumbnailTarget = null;
}

async function openCollectionThumbnailDialog(collection) {
    const collectionVideos = await invoke('get_collection_videos', { collectionId: collection.id });
    closeVideoMenus();
    collectionThumbnailTarget = collection;
    document.getElementById('collection-thumbnail-title').textContent = `Choose a cover image for "${collection.name}"`;
    document.getElementById('dialog-collection-thumbnail').style.display = 'flex';
    document.getElementById('collection-thumbnail-clear').style.display = (collection.cover_video_id || collection.cover_image_path) ? 'inline-flex' : 'none';
    renderCollectionThumbnailGrid(collectionVideos, collection.cover_video_id);
}

async function uploadCollectionThumbnailImage() {
    if (!collectionThumbnailTarget) return;

    try {
        const { open } = window.__TAURI__.dialog ||
            await import('@tauri-apps/plugin-dialog');

        const selected = await open({
            multiple: false,
            directory: false,
            title: 'Choose Collection Cover Image',
            filters: [{
                name: 'Images',
                extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp', 'svg'],
            }],
        });

        if (!selected) return;

        const sourceImagePath = typeof selected === 'string' ? selected : selected.path;
        await invoke('set_collection_cover_image', {
            collectionId: collectionThumbnailTarget.id,
            sourceImagePath,
        });

        delete collectionCoverCache[collectionThumbnailTarget.id];
        await refreshCollectionsState();
        const updated = collections.find(c => c.id === collectionThumbnailTarget.id);
        collectionThumbnailTarget = updated || collectionThumbnailTarget;
        await loadCollectionCoversBatch();
        const refreshedVideos = await invoke('get_collection_videos', { collectionId: collectionThumbnailTarget.id });
        renderCollectionThumbnailGrid(refreshedVideos, null);
        document.getElementById('collection-thumbnail-clear').style.display = 'inline-flex';
        showToast('Collection cover image uploaded', 'success');
    } catch (e) {
        if (!String(e).includes('cancelled') && !String(e).includes('user')) {
            showToast('Failed to upload collection cover', 'error');
        }
    }
}

function renderCollectionThumbnailGrid(videos, activeCoverId) {
    const grid = document.getElementById('collection-thumbnail-grid');
    if (videos.length === 0) {
        grid.innerHTML = '<p class="collection-picker-empty">No videos in this collection yet. You can still upload any image as the cover.</p>';
        return;
    }

    grid.innerHTML = videos.map(video => {
        const thumbnail = thumbnailCache[video.id];
        return `
            <button type="button" class="collection-thumbnail-option ${video.id === activeCoverId ? 'active' : ''}" data-thumbnail-video-id="${video.id}">
                <div class="collection-thumbnail-media">
                    ${thumbnail
                        ? `<img src="${thumbnail}" alt="${escapeHtml(video.title)}" />`
                        : `<div class="collection-thumbnail-placeholder"><svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="0.75"><rect x="2" y="4" width="20" height="16" rx="3"/><polygon points="10,8 16,12 10,16"/></svg></div>`
                    }
                </div>
                <div class="collection-thumbnail-label">${escapeHtml(video.title)}</div>
            </button>
        `;
    }).join('');

    grid.querySelectorAll('[data-thumbnail-video-id]').forEach(btn => {
        btn.addEventListener('click', async () => {
            if (!collectionThumbnailTarget) return;

            try {
                await invoke('set_collection_cover', {
                    collectionId: collectionThumbnailTarget.id,
                    coverVideoId: btn.dataset.thumbnailVideoId,
                });
                await refreshCollectionsState();
                const updated = collections.find(c => c.id === collectionThumbnailTarget.id);
                collectionThumbnailTarget = updated || collectionThumbnailTarget;
                const refreshedVideos = await invoke('get_collection_videos', { collectionId: collectionThumbnailTarget.id });
                renderCollectionThumbnailGrid(refreshedVideos, btn.dataset.thumbnailVideoId);
                document.getElementById('collection-thumbnail-clear').style.display = 'inline-flex';
                showToast('Collection thumbnail updated', 'success');
            } catch (e) {
                showToast('Failed to update collection thumbnail', 'error');
            }
        });
    });
}

async function clearCollectionThumbnail() {
    if (!collectionThumbnailTarget) return;

    try {
        await invoke('set_collection_cover', {
            collectionId: collectionThumbnailTarget.id,
            coverVideoId: null,
        });
        delete collectionCoverCache[collectionThumbnailTarget.id];
        await refreshCollectionsState();
        const updated = collections.find(c => c.id === collectionThumbnailTarget.id);
        collectionThumbnailTarget = updated || collectionThumbnailTarget;
        const refreshedVideos = await invoke('get_collection_videos', { collectionId: collectionThumbnailTarget.id });
        renderCollectionThumbnailGrid(refreshedVideos, null);
        document.getElementById('collection-thumbnail-clear').style.display = 'none';
        showToast('Collection thumbnail removed', 'success');
    } catch (e) {
        showToast('Failed to remove collection thumbnail', 'error');
    }
}

async function openCollectionPickerDialog(video) {
    closeVideoMenus();
    collectionPickerVideo = video;
    document.getElementById('collection-picker-video-title').textContent = video.title;
    document.getElementById('dialog-collection-picker').style.display = 'flex';
    document.getElementById('collection-picker-name').value = '';
    await renderCollectionPickerDialog(video);
    document.getElementById('collection-picker-name').focus();
}

async function renderCollectionPickerDialog(video) {
    const container = document.getElementById('collection-picker-list');

    if (collections.length === 0) {
        container.innerHTML = '<p class="collection-picker-empty">No collections yet. Create one below and this video will be added immediately.</p>';
        return;
    }

    const collectionStates = await Promise.all(collections.map(async (col) => {
        try {
            const colVideos = await invoke('get_collection_videos', { collectionId: col.id });
            return {
                collection: col,
                isInCollection: colVideos.some(v => v.id === video.id),
            };
        } catch (e) {
            return {
                collection: col,
                isInCollection: false,
            };
        }
    }));

    container.innerHTML = collectionStates.map(({ collection, isInCollection }) => `
        <div class="collection-picker-item">
            <div class="collection-picker-body">
                <div class="collection-picker-name">${escapeHtml(collection.name)}</div>
                <div class="collection-picker-meta">${collection.video_count} videos</div>
            </div>
            <button class="collection-picker-button ${isInCollection ? 'added' : ''}" data-picker-collection-id="${collection.id}" data-picker-action="${isInCollection ? 'remove' : 'add'}">
                ${isInCollection ? 'Added' : 'Add'}
            </button>
        </div>
    `).join('');

    container.querySelectorAll('[data-picker-collection-id]').forEach(btn => {
        btn.addEventListener('click', async () => {
            if (!collectionPickerVideo) return;

            const collectionId = btn.dataset.pickerCollectionId;
            const action = btn.dataset.pickerAction;

            try {
                if (action === 'add') {
                    await invoke('add_video_to_collection', { collectionId, videoId: collectionPickerVideo.id });
                    showToast('Added to collection', 'success');
                } else {
                    await invoke('remove_video_from_collection', { collectionId, videoId: collectionPickerVideo.id });
                    showToast('Removed from collection', 'success');
                }

                await refreshCollectionsState();
                await renderCollectionPickerDialog(collectionPickerVideo);
                if (selectedVideo && selectedVideo.id === collectionPickerVideo.id) {
                    renderModalCollections(selectedVideo);
                }
            } catch (e) {
                showToast('Failed to update collection', 'error');
            }
        });
    });
}

async function createCollectionFromPicker() {
    if (!collectionPickerVideo) return;

    const name = document.getElementById('collection-picker-name').value.trim();
    if (!name) {
        showToast('Please enter a collection name', 'error');
        return;
    }

    try {
        const col = await invoke('create_collection', { name, description: '' });
        await invoke('add_video_to_collection', { collectionId: col.id, videoId: collectionPickerVideo.id });
        document.getElementById('collection-picker-name').value = '';
        showToast(`Created "${name}" and added the video`, 'success');
        await refreshCollectionsState();
        await renderCollectionPickerDialog(collectionPickerVideo);
        if (selectedVideo && selectedVideo.id === collectionPickerVideo.id) {
            renderModalCollections(selectedVideo);
        }
    } catch (e) {
        showToast('Failed to create collection', 'error');
    }
}

async function toggleWatchedModal() {
    if (!selectedVideo) return;
    const newState = !selectedVideo.watched;
    try {
        await invoke('set_video_watched', { videoId: selectedVideo.id, watched: newState });
        selectedVideo.watched = newState ? 1 : 0;
        const idx = allVideos.findIndex(v => v.id === selectedVideo.id);
        if (idx >= 0) allVideos[idx].watched = selectedVideo.watched;
        updateWatchedButton(selectedVideo);
        updatePlayButton(selectedVideo);
        renderCurrentView();
        showToast(newState ? 'Marked as watched' : 'Marked as unwatched', 'success');
    } catch (e) {
        showToast('Failed to update watched state', 'error');
    }
}

async function toggleFavoriteModal() {
    if (!selectedVideo) return;
    const newState = selectedVideo.favorite !== 1;
    try {
        await invoke('set_video_favorite', { videoId: selectedVideo.id, favorite: newState });
        selectedVideo.favorite = newState ? 1 : 0;
        const idx = allVideos.findIndex(v => v.id === selectedVideo.id);
        if (idx >= 0) allVideos[idx].favorite = selectedVideo.favorite;
        document.getElementById('modal-favorite').classList.toggle('active', newState);
        renderCurrentView();
        showToast(newState ? 'Added to favorites' : 'Removed from favorites', 'success');
    } catch (e) {
        showToast('Failed to update favorite', 'error');
    }
}

async function renderModalCollections(video) {
    const container = document.getElementById('modal-collection-list');
    if (collections.length === 0) {
        container.innerHTML = '<p style="font-size: 12px; color: var(--text-tertiary);">No collections yet. Create one from the sidebar.</p>';
        return;
    }

    // For each collection, check if this video is in it
    let html = '';
    for (const col of collections) {
        try {
            const colVideos = await invoke('get_collection_videos', { collectionId: col.id });
            const isInCollection = colVideos.some(v => v.id === video.id);
            html += `
                <div class="modal-collection-item">
                    <span>${escapeHtml(col.name)}</span>
                    <button class="${isInCollection ? 'added' : ''}" data-collection-id="${col.id}" data-video-id="${video.id}" data-action="${isInCollection ? 'remove' : 'add'}">
                        ${isInCollection ? '✓ Added' : '+ Add'}
                    </button>
                </div>
            `;
        } catch (e) {
            // skip
        }
    }
    container.innerHTML = html;

    // Attach listeners
    container.querySelectorAll('button').forEach(btn => {
        btn.addEventListener('click', async () => {
            const colId = btn.dataset.collectionId;
            const vidId = btn.dataset.videoId;
            const action = btn.dataset.action;
            try {
                if (action === 'add') {
                    await invoke('add_video_to_collection', { collectionId: colId, videoId: vidId });
                    btn.textContent = '✓ Added';
                    btn.classList.add('added');
                    btn.dataset.action = 'remove';
                } else {
                    await invoke('remove_video_from_collection', { collectionId: colId, videoId: vidId });
                    btn.textContent = '+ Add';
                    btn.classList.remove('added');
                    btn.dataset.action = 'add';
                }
                collections = await invoke('get_collections');
                renderCollectionNav();
            } catch (e) {
                showToast('Failed to update collection', 'error');
            }
        });
    });
}

// ─── Library Management ───
async function addLibrary() {
    try {
        const { open } = window.__TAURI__.dialog ||
            await import('@tauri-apps/plugin-dialog');
        
        const selected = await open({
            directory: true,
            multiple: false,
            title: 'Select Video Library Folder',
        });

        if (selected) {
            const path = typeof selected === 'string' ? selected : selected.path;
            // Derive name from folder name
            const name = path.split(/[\\/]/).filter(Boolean).pop() || 'Library';
            
            showLoading('Adding library...');
            const lib = await invoke('add_library', { path, name });
            libraries.push(lib);
            renderLibraryNav();
            renderSettings();
            showToast(`Library "${name}" added`, 'success');
            
            // Auto-scan
            await scanLibrary(lib.id, lib.path);
            hideLoading();
        }
    } catch (e) {
        hideLoading();
        if (!String(e).includes('cancelled') && !String(e).includes('user')) {
            showToast('Failed to add library: ' + e, 'error');
        }
    }
}

async function removeLibrary(libraryId) {
    try {
        await invoke('remove_library', { libraryId });
        libraries = libraries.filter(l => l.id !== libraryId);
        allVideos = allVideos.filter(v => v.library_id !== libraryId);
        renderLibraryNav();
        renderSettings();
        renderCurrentView();
        updateStats();
        showToast('Library removed', 'success');
    } catch (e) {
        showToast('Failed to remove library', 'error');
    }
}

async function scanLibrary(libraryId, libraryPath) {
    showLoading('Scanning library...');
    try {
        const videos = await invoke('scan_library', { libraryId, libraryPath });
        // Update all videos
        allVideos = await invoke('get_all_videos');
        renderCurrentView();
        updateStats();
        showToast(`Found ${videos.length} videos`, 'success');
        
        // Generate thumbnails in background
        hideLoading();
        showToast('Generating thumbnails...', 'success');
        const count = await invoke('generate_thumbnails');
        if (count > 0) {
            allVideos = await invoke('get_all_videos');
            renderCurrentView();
            loadThumbnailsBatch();
            showToast(`Generated ${count} thumbnails`, 'success');
        }
    } catch (e) {
        hideLoading();
        showToast('Scan failed: ' + e, 'error');
    }
}

async function scanAllLibraries() {
    showLoading('Scanning all libraries...');
    try {
        const btn = document.getElementById('btn-refresh');
        btn.classList.add('spinning');
        
        allVideos = await invoke('scan_all_libraries');
        renderCurrentView();
        updateStats();
        showToast(`Library updated: ${allVideos.length} videos`, 'success');
        
        hideLoading();
        
        // Generate thumbnails
        const count = await invoke('generate_thumbnails');
        if (count > 0) {
            allVideos = await invoke('get_all_videos');
            renderCurrentView();
            loadThumbnailsBatch();
        }
        
        btn.classList.remove('spinning');
    } catch (e) {
        hideLoading();
        document.getElementById('btn-refresh').classList.remove('spinning');
        showToast('Scan failed: ' + e, 'error');
    }
}

async function refreshLibrary() {
    await scanAllLibraries();
}

// ─── Playback ───
async function playVideo(video) {
    try {
        closeVideoMenus();
        const resumeSecs = getResumeSeconds(video);
        const result = await invoke('open_in_player', {
            videoId: video.id,
            videoPath: video.path,
            resumeSecs,
        });

        if (result.already_running) {
            showToast(result.message, 'info');
            return;
        }

        if (result.tracking_enabled) {
            showToast(
                result.resumed
                    ? `Resuming in VLC from ${formatDuration(result.resume_position_secs)}...`
                    : 'Opening in VLC with progress tracking...',
                'success'
            );
        } else {
            showToast(result.message || 'Opening in player...', result.fallback_used ? 'info' : 'success');
        }

        setTimeout(() => {
            syncVideoState();
        }, 1500);
    } catch (e) {
        showToast('Failed to open video: ' + e, 'error');
    }
}

// ─── Collections ───
function showNewCollectionDialog() {
    document.getElementById('dialog-new-collection').style.display = 'flex';
    document.getElementById('input-collection-name').value = '';
    document.getElementById('input-collection-desc').value = '';
    document.getElementById('input-collection-name').focus();
}

async function createCollection() {
    const name = document.getElementById('input-collection-name').value.trim();
    const desc = document.getElementById('input-collection-desc').value.trim();
    if (!name) {
        showToast('Please enter a collection name', 'error');
        return;
    }
    try {
        const col = await invoke('create_collection', { name, description: desc });
        collections.push(col);
        renderCollectionNav();
        document.getElementById('dialog-new-collection').style.display = 'none';
        showToast(`Collection "${name}" created`, 'success');
    } catch (e) {
        showToast('Failed to create collection', 'error');
    }
}

// ─── Sidebar Rendering ───
function renderLibraryNav() {
    const container = document.getElementById('library-nav-list');
    container.innerHTML = libraries.map(lib => `
        <div class="nav-item nav-item-library" data-library-id="${lib.id}" data-view="library" title="${escapeHtml(lib.name)}" role="button" tabindex="0">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M22 19a2 2 0 01-2 2H4a2 2 0 01-2-2V5a2 2 0 012-2h5l2 3h9a2 2 0 012 2z"/></svg>
            <span>${escapeHtml(lib.name)}</span>
            <button type="button" class="nav-delete" title="Remove" data-library-id="${lib.id}">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M18 6L6 18M6 6l12 12"/></svg>
            </button>
        </div>
    `).join('');

    container.querySelectorAll('.nav-item-library').forEach(btn => {
        btn.addEventListener('click', () => {
            switchView('library', { libraryId: btn.dataset.libraryId });
        });
        btn.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                switchView('library', { libraryId: btn.dataset.libraryId });
            }
        });
    });

    container.querySelectorAll('.nav-delete').forEach(btn => {
        btn.addEventListener('click', (e) => {
            e.stopPropagation();
            removeLibrary(btn.dataset.libraryId);
        });
    });
}

function renderCollectionNav() {
    const container = document.getElementById('collection-nav-list');
    container.innerHTML = collections.map(col => `
        <div class="nav-item nav-item-collection" data-collection-id="${col.id}" data-view="collection" title="${escapeHtml(col.name)}" role="button" tabindex="0">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><rect x="2" y="7" width="20" height="14" rx="2"/><path d="M16 3h-8l-2 4h12l-2-4z"/></svg>
            <span>${escapeHtml(col.name)}</span>
            <button type="button" class="nav-delete" title="Delete" data-collection-id="${col.id}">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M18 6L6 18M6 6l12 12"/></svg>
            </button>
        </div>
    `).join('');

    container.querySelectorAll('.nav-item-collection').forEach(btn => {
        btn.addEventListener('click', () => {
            switchView('collection', { collectionId: btn.dataset.collectionId });
        });
        btn.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                switchView('collection', { collectionId: btn.dataset.collectionId });
            }
        });
    });

    container.querySelectorAll('.nav-delete').forEach(btn => {
        btn.addEventListener('click', async (e) => {
            e.stopPropagation();
            try {
                await invoke('delete_collection', { collectionId: btn.dataset.collectionId });
                collections = collections.filter(c => c.id !== btn.dataset.collectionId);
                renderCollectionNav();
                if (currentView === 'collection' && currentCollectionId === btn.dataset.collectionId) {
                    switchView('home');
                }
                showToast('Collection deleted', 'success');
            } catch (e) {
                showToast('Failed to delete collection', 'error');
            }
        });
    });
}

async function updateStats() {
    try {
        const stats = await invoke('get_library_stats');
        const statsEl = document.getElementById('sidebar-stats');
        statsEl.innerHTML = `
            ${stats.total_videos} videos · ${formatFileSize(stats.total_size_bytes)}<br/>
            ${stats.watched_videos} watched · ${stats.unwatched_videos} unwatched
        `;
    } catch (e) {
        // ignore
    }
}

// ─── Filtering & Sorting ───
function filterAndSort(videos) {
    let filtered = [...videos];

    // Search filter
    if (searchQuery) {
        filtered = filtered.filter(v =>
            v.title.toLowerCase().includes(searchQuery) ||
            v.file_name.toLowerCase().includes(searchQuery)
        );
    }

    // Sort
    switch (sortMode) {
        case 'date-desc':
            filtered.sort((a, b) => b.date_added.localeCompare(a.date_added));
            break;
        case 'date-asc':
            filtered.sort((a, b) => a.date_added.localeCompare(b.date_added));
            break;
        case 'title-asc':
            filtered.sort((a, b) => a.title.localeCompare(b.title));
            break;
        case 'title-desc':
            filtered.sort((a, b) => b.title.localeCompare(a.title));
            break;
        case 'size-desc':
            filtered.sort((a, b) => b.file_size - a.file_size);
            break;
        case 'size-asc':
            filtered.sort((a, b) => a.file_size - b.file_size);
            break;
        case 'duration-desc':
            filtered.sort((a, b) => (b.duration_secs || 0) - (a.duration_secs || 0));
            break;
        case 'duration-asc':
            filtered.sort((a, b) => (a.duration_secs || 0) - (b.duration_secs || 0));
            break;
    }

    return filtered;
}

// ─── Utilities ───
function formatDuration(secs) {
    if (!secs) return '';
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = Math.floor(secs % 60);
    if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
    return `${m}:${String(s).padStart(2, '0')}`;
}

function formatFileSize(bytes) {
    if (!bytes) return '';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let i = 0;
    let size = bytes;
    while (size >= 1024 && i < units.length - 1) {
        size /= 1024;
        i++;
    }
    return `${size.toFixed(i > 1 ? 1 : 0)} ${units[i]}`;
}

function escapeHtml(str) {
    if (!str) return '';
    return str
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#039;');
}

// ─── Toast Notifications ───
function showToast(message, type = 'info') {
    const container = document.getElementById('toast-container');
    const toast = document.createElement('div');
    toast.className = `toast ${type}`;
    toast.textContent = message;
    container.appendChild(toast);

    setTimeout(() => {
        toast.classList.add('toast-exit');
        setTimeout(() => toast.remove(), 300);
    }, 3000);
}

// ─── Loading ───
function showLoading(text = 'Loading...') {
    document.getElementById('loading-text').textContent = text;
    document.getElementById('loading-overlay').style.display = 'flex';
}

function hideLoading() {
    document.getElementById('loading-overlay').style.display = 'none';
}

// ─── Scroll Reveal ───
function setupScrollReveal() {
    const observer = new IntersectionObserver((entries) => {
        entries.forEach((entry, index) => {
            if (entry.isIntersecting) {
                setTimeout(() => {
                    entry.target.classList.add('visible');
                }, index * 50);
                observer.unobserve(entry.target);
            }
        });
    }, {
        threshold: 0.1,
        rootMargin: '0px 0px -20px 0px',
    });

    window._revealObserver = observer;
}

function applyScrollReveal() {
    requestAnimationFrame(() => {
        document.querySelectorAll('.reveal-item:not(.visible)').forEach(el => {
            window._revealObserver.observe(el);
        });
    });
}
